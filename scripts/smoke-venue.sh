#!/usr/bin/env bash
# End-to-end smoke: the REAL `gravity` CLI against a REAL prism gateway, with
# k10r's mem-server underneath and a stub SSE provider standing in for the LLM.
#
# The unit tests pin the client's side against a stub venue, and prism's
# `turn_e2e` pins the server's side in-process. Neither one puts the shipped
# binary on a shipped gateway, which is the only thing this script claims to do:
#   1. stub provider (SSE)       — fixed tokens, so the expected reply is known
#   2. krystallizer mem-server   — a fresh store under $ROOT
#   3. prism                     — open posture, gravity's booth binaries wired
#   4. `gravity --json hi`       — the CLI, as a user would run it, with the
#                                  stub asking for `commit_memory` on round 0
# and then reads k10r back to prove the turn landed (a reply on stdout is not
# the same claim as a written record).
#
# Everything is built debug, so `cargo build` the three repos first.
#
#   GRAVITY_REPO / PRISM_REPO / K10R_REPO   repo roots (default ~/world/<name>)
#   ROOT                                    scratch root (default $TMPDIR/smoke-venue)
#   SSE_PORT / MEM_PORT / PRISM_PORT        fixed ports, so a human can poke them
#   EXPECTED                                the reply asserted (default "hello world")
set -uo pipefail

GRAVITY_REPO=${GRAVITY_REPO:-$HOME/world/gravity}
PRISM_REPO=${PRISM_REPO:-$HOME/world/prism}
K10R_REPO=${K10R_REPO:-$HOME/world/krystallizer}
EFFECTOR_REPO=${EFFECTOR_REPO:-$HOME/world/effector}
ROOT=${ROOT:-${TMPDIR:-/tmp}/smoke-venue}
SSE_PORT=${SSE_PORT:-18801}
MEM_PORT=${MEM_PORT:-17760}
PRISM_PORT=${PRISM_PORT:-18765}
EXPECTED=${EXPECTED:-hello world}

fail() {
    echo "FAIL: $*"
    exit 1
}

[ -x "$GRAVITY_REPO/target/debug/gravity" ] || fail "build gravity first (cargo build -p gravity-cli -p gravity-booth)"
[ -x "$PRISM_REPO/target/debug/prism" ] || fail "build prism first"
[ -x "$K10R_REPO/target/debug/krystallizer" ] || fail "build krystallizer first"
[ -x "$EFFECTOR_REPO/target/debug/effector" ] || fail "build effector first (cargo build -p effector-runtime)"
NU_RUNNER="$EFFECTOR_REPO/deploy/runners/runner_nu.nu"
# The RESIDENT form (SMOKE_PY=1): the same role, loaded once by the node's embedded
# python carrier instead of spawned per call — the one shape in which a tool call can be
# warm (gravity ADR-0014 §5).
PY_RUNNER="$EFFECTOR_REPO/deploy/runners/runner_py_guest.py"
[ -f "$NU_RUNNER" ] || fail "no nu runner at $NU_RUNNER"

MODEL_DIR=${KRYSTALLIZER_MODEL_DIR:-$HOME/.local/share/krystallizer/models/bge-small-zh-v1.5}
[ -d "$MODEL_DIR" ] || fail "no embedding model at $MODEL_DIR (k10r cannot boot without one)"

rm -rf "$ROOT"
mkdir -p "$ROOT/data" "$ROOT/prism-data"

cat > "$ROOT/krystallizer.kdl" <<KDL
storage {
    data-dir "$ROOT/data"
}
embedding {
    mode "local"
    model-dir "$MODEL_DIR"
    dimensions 512
    metric "cosine"
}
compression {
    context-bytes 131072
    budget-percent 50
    cache-ttl-ms 300000
    floor-bytes 2048
}
logs {
    max-rows 1000
}
KDL

cat > "$ROOT/sse.py" <<PY
import json
import os
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1])
CHUNKS = ["hello", " world"]
# One fact the model is told to commit, so the run exercises the TOOL PATH
# through the real stack (prism -> the booth -> k10r), not just the reply path.
FACT = [["alice", "person", {}, "likes", {}, "tea", "drink", {}]]
ROUNDS = 0
LOCK = threading.Lock()
# The code row's id, seeded by the script before the turn: a file rather than an argv so the id
# is not a second copy of the seed in two places. Read PER REQUEST — the provider starts before
# the row exists.
CODE_FILE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "code_id")


def code_id() -> int:
    with open(CODE_FILE) as handle:
        return int(handle.read())


# The RESIDENT leg (SMOKE_PY=1): a python row the turn calls TWICE, so the second answer can only
# be right if the row's own globals survived between the calls (gravity ADR-0014 §5). Absent file
# = the leg is off.
PY_CODE_FILE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "py_code_id")


def py_code_id():
    if not os.path.exists(PY_CODE_FILE):
        return None
    with open(PY_CODE_FILE) as handle:
        return int(handle.read())


# The `seen` counters the provider has been shown, in order. Each ROUND carries only the latest
# tool exchange, so the two answers of one turn are never in one request — the memory has to be
# here, and what it checks is the same thing: consecutive, i.e. the row's globals survived.
SEEN_VALUES = []
# What the node COMPUTED, recorded as it passes through: the check cannot read it off the final
# request, because a later round's replay carries only the LATEST tool exchange.
TOTALS = []


def _numbers(doc, key, out):
    """Every `key: <number>` anywhere in the request, tool-message strings included.

    A tool answer arrives as a JSON string INSIDE the request, so its quotes are escaped
    and a byte-pattern search for `"seen":` finds nothing. Walking the parsed document —
    and re-parsing a string that looks like JSON — is the only reading that is not a
    guess about quoting.
    """
    if isinstance(doc, dict):
        for name, value in doc.items():
            if name == key and isinstance(value, (int, float)) and not isinstance(value, bool):
                out.append(int(value))
            else:
                _numbers(value, key, out)
    elif isinstance(doc, list):
        for item in doc:
            _numbers(item, key, out)
    elif isinstance(doc, str) and doc.strip()[:1] in ("{", "["):
        try:
            _numbers(json.loads(doc), key, out)
        except Exception:
            pass


def note_seen(body):
    try:
        doc = json.loads(body)
    except Exception:
        return
    found = []
    _numbers(doc, "seen", found)
    for number in found:
        if not SEEN_VALUES or SEEN_VALUES[-1] != number:
            SEEN_VALUES.append(number)
    _numbers(doc, "total", TOTALS)


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_POST(self):
        global ROUNDS
        body = self.rfile.read(int(self.headers.get("content-length", 0)))
        # The affordances must ride the request (ADR-0004): a model that is
        # never told what it can call cannot call anything. A missing list is
        # answered as a server error so the turn fails loudly instead of
        # quietly looking like a model that chose not to use a tool.
        missing = [name for name in (b"recall_memory", b"commit_memory") if name not in body]
        if missing:
            self.send_response(500)
            self.send_header("content-length", "0")
            self.end_headers()
            return
        # A NEW TURN STARTS AT ROUND 0: the request carries the conversation, and a fresh turn is
        # the one whose messages hold no tool answer yet. Without this the page's turn would fall
        # into the tail of the CLI's round sequence and never call a tool — the smoke would pass
        # while proving nothing about a page.
        if b'"tool"' not in body:
            with LOCK:
                ROUNDS = 0
        with LOCK:
            round_index = ROUNDS
            ROUNDS += 1
        note_seen(body)
        # round 1's check needs the body, and a refusal must not start a 200.
        if round_index == 1 and b"github-issues" not in body:
            self.send_response(500)
            self.send_header("content-length", "0")
            self.end_headers()
            return
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("cache-control", "no-cache")
        self.send_header("transfer-encoding", "chunked")
        self.end_headers()

        def chunk(payload: bytes) -> None:
            self.wfile.write(b"%x\r\n%s\r\n" % (len(payload), payload))
            self.wfile.flush()

        def event(delta: dict) -> None:
            chunk(b"data: " + json.dumps({"choices": [{"index": 0, "delta": delta}]}).encode() + b"\n\n")

        if round_index == 0:
            # A real model with the list in front of it asks for a capability
            # first: resolve_skills, the group that lists what this deployment
            # can do.
            event(
                {
                    "tool_calls": [
                        {
                            "id": "c1",
                            "function": {
                                "name": "resolve_skills",
                                "arguments": json.dumps({"query": "list github issues"}),
                            },
                        }
                    ]
                }
            )
        elif round_index == 2:
            # THE NODE (gravity ADR-0001, Phase 4.6): a code row runs on the mount that serves
            # its language. The id comes from the row seeded before the run, and the arguments
            # are the row's OWN declared schema — checked before a node is asked anything.
            event(
                {
                    "tool_calls": [
                        {
                            "id": "c3",
                            "function": {
                                "name": "run",
                                "arguments": json.dumps(
                                    {"code_id": code_id(), "arguments": {"n": 4}}
                                ),
                            },
                        }
                    ]
                }
            )
        elif round_index == 3 and py_code_id() is not None:
            # THE SAME ROW, TWICE. A one-shot runner would answer 1 and 1; a resident one answers
            # consecutive numbers, because the row is not recompiled and its namespace is kept.
            event(
                {
                    "tool_calls": [
                        {
                            "id": "c4",
                            "function": {
                                "name": "run",
                                "arguments": json.dumps(
                                    {"code_id": py_code_id(), "arguments": {"by": 1}}
                                ),
                            },
                        }
                    ]
                }
            )
        elif round_index == 4 and py_code_id() is not None:
            event(
                {
                    "tool_calls": [
                        {
                            "id": "c5",
                            "function": {
                                "name": "run",
                                "arguments": json.dumps(
                                    {"code_id": py_code_id(), "arguments": {"by": 1}}
                                ),
                            },
                        }
                    ]
                }
            )
        elif round_index == (5 if py_code_id() is not None else 3):
            # What the node COMPUTED has to be in this request as the tool message: a turn that
            # answered without it would look identical from the outside. A refusal keeps the
            # request on disk, because "the model did not use the tool" and "the call failed with
            # words" look the same in a 500.
            if py_code_id() is not None and (
                len(SEEN_VALUES) < 2 or SEEN_VALUES[-1] != SEEN_VALUES[-2] + 1
            ):
                print(f"resident leg: seen={SEEN_VALUES} totals={TOTALS}", flush=True)
                with open(os.path.join(os.path.dirname(CODE_FILE), "round5.json"), "wb") as out:
                    out.write(body)
                self.send_response(500)
                self.send_header("content-length", "0")
                self.end_headers()
                return
            if 7 not in TOTALS:
                print(f"node leg: totals={TOTALS}", flush=True)
                with open(os.path.join(os.path.dirname(CODE_FILE), "round3.json"), "wb") as out:
                    out.write(body)
                self.send_response(500)
                self.send_header("content-length", "0")
                self.end_headers()
                return
            for piece in CHUNKS:
                event({"content": piece})
            chunk(b"data: [DONE]\n\n")
            chunk(b"")
            return
        elif round_index == 1:
            # The resolver's answer must have come BACK as the tool message of
            # this request, carrying the skill declared before the run. A door
            # that answers into the void looks exactly like a model that never
            # asked, so the check is here rather than in k10r.
            event(
                {
                    "tool_calls": [
                        {
                            "id": "c2",
                            "function": {
                                "name": "commit_memory",
                                "arguments": json.dumps({"facts": FACT}),
                            },
                        }
                    ]
                }
            )
        else:
            for piece in CHUNKS:
                event({"content": piece})
        chunk(b"data: [DONE]\n\n")
        chunk(b"")

    def log_message(self, *args):
        pass


ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
PY

PIDS=""
cleanup() {
    for pid in $PIDS; do kill "$pid" 2>/dev/null; done
    wait 2>/dev/null
}
trap cleanup EXIT

wait_for() {
    local port=$1 name=$2 tries=0
    until (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; do
        tries=$((tries + 1))
        [ "$tries" -gt 120 ] && fail "$name never listened on $port"
        sleep 0.25
    done
    exec 3<&- 3>&-
    echo "ok: $name on $port"
}

python3 "$ROOT/sse.py" "$SSE_PORT" >"$ROOT/sse.log" 2>&1 &
PIDS="$PIDS $!"
wait_for "$SSE_PORT" "stub provider" || exit 1

"$K10R_REPO/target/debug/krystallizer" --config "$ROOT/krystallizer.kdl" --addr "127.0.0.1:$MEM_PORT" \
    >"$ROOT/mem.log" 2>&1 &
PIDS="$PIDS $!"
wait_for "$MEM_PORT" "mem-server" || exit 1

# One skill declared before the turn: `resolve_skills` must have something to find, or the smoke
# proves the door exists and nothing else. Its step names the code row declared below — the
# resolver does not walk it here (`run` addresses the row by id), so the id in this row is the
# skill's own business and stays a placeholder.
# THE CODE ROW the mount will run (k10r's `code` rows are the graph's citizens; a skill's step
# names one by id). Its `parameters` is the schema the caller's arguments are checked against
# BEFORE a node is asked anything — the row is the contract, not the script's good manners.
curl -sf -X POST "http://127.0.0.1:$MEM_PORT/code/37" \
    -H 'content-type: application/json' \
    -d '{"language":"nu","description":"sum four more","parameters":"{\"type\":\"object\",\"properties\":{\"n\":{\"type\":\"number\"}},\"required\":[\"n\"]}","source":"{total: (($env.TOOLRUNNER_ARGS | from json | get n) + 3)} | to json"}' \
    >"$ROOT/code.json" || fail "declaring a code row failed"
python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["code_id"])' "$ROOT/code.json" >"$ROOT/code_id" \
    || fail "the code row answered no id: $(cat "$ROOT/code.json")"
echo "ok: a code row is declared for the mount to run (id $(cat "$ROOT/code_id"))"

# THE RESIDENT ROW (SMOKE_PY=1): it accumulates in a module global, so its SECOND call proves
# whether the node kept the compiled row or compiled it again (gravity ADR-0014 §5).
if [ "${SMOKE_PY:-0}" = 1 ]; then
    # SAME SCOPE as the nu row (the path segment is the scope, not an id — the turn reads
    # `/code/<scope>/<id>`, so a row seeded under another scope is invisible to it).
    curl -sf -X POST "http://127.0.0.1:$MEM_PORT/code/37" \
        -H 'content-type: application/json' \
        -d '{"language":"python","description":"count the calls it has seen","parameters":"{\"type\":\"object\",\"properties\":{\"by\":{\"type\":\"number\"}},\"required\":[\"by\"]}","source":"STATE[\"seen\"] = STATE.get(\"seen\", 0) + ARGS.get(\"by\", 1)\nprint(json.dumps({\"seen\": STATE[\"seen\"]}))\n"}' \
        >"$ROOT/py_code.json" || fail "declaring the python code row failed"
    python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["code_id"])' "$ROOT/py_code.json" >"$ROOT/py_code_id" \
        || fail "the python code row answered no id: $(cat "$ROOT/py_code.json")"
    echo "ok: a python code row is declared for the resident leg (id $(cat "$ROOT/py_code_id"))"
fi

curl -sf -X POST "http://127.0.0.1:$MEM_PORT/skills/37" \
    -H 'content-type: application/json' \
    -d '{"name":"github-issues","title":"issues","content":"Lists the open issues of a GitHub repository.","steps":"[{\"code_id\":1}]"}' \
    >"$ROOT/skill.json" || fail "declaring a skill failed"
grep -q '"created":true' "$ROOT/skill.json" || fail "the skill was not created: $(cat "$ROOT/skill.json")"
echo "ok: a skill is declared for the resolver to find"

# Only the TWO SPAWN SPECS are environment here: where the guest binaries live. The deployment
# facts (store URL, model, endpoint, key env, memory scope) are ROWS in the turn type's namespace
# now (gravity ADR-0007), written below through the application's own door — a gateway that served
# this app learned no model vocabulary.
# THE DEPLOYMENT'S LANGUAGE VOCABULARY (prism ADR-0022): one entry, and a mount is the product of
# it and an approved node — in `open` posture, of a node that DIALS (ADR-0022 §5). The runner is a
# fixed artifact delivered BY REFERENCE (ADR-0027), which is why the venue also serves `/code`.
# `requires: host` because this node is BARE METAL (`class: host` below): the default is the
# container, and a host node cannot become one on demand — the plane refuses in words rather than
# quietly running unreviewed code in the wrong isolation (ADR-0001 §4).
if [ "${SMOKE_PY:-0}" = 1 ]; then
    # The runner IS the source for an embedded carrier (prism ADR-0021: a mount's runner is
    # a fixed artifact the deployment declares), so the module's TEXT is what the vocabulary
    # carries — read from disk here rather than pasted into an env var.
    PRISM_LANGUAGES=$(python3 - "$NU_RUNNER" "$PY_RUNNER" <<'PY'
import json, sys
nu, py = sys.argv[1], sys.argv[2]
print(json.dumps([
    {"language": "nu", "carrier": "exec", "runner": f"nu {nu}", "requires": "host"},
    {"language": "python", "carrier": "induction", "runner": f"python3 {py}", "requires": "host"},
]))
PY
)
else
    PRISM_LANGUAGES="[{\"language\":\"nu\",\"carrier\":\"exec\",\"runner\":\"nu $NU_RUNNER\",\"requires\":\"host\"}]"
fi
PRISM_LANGUAGES="$PRISM_LANGUAGES" \
PRISM_ADDR="127.0.0.1:$PRISM_PORT" PRISM_IDENTITY=open PRISM_DATA="$ROOT/prism-data" \
GRAVITY_PROVIDER_BIN="$GRAVITY_REPO/target/debug/gravity-provider" \
GRAVITY_TURN_BIN="$GRAVITY_REPO/target/debug/gravity-turn" \
SMOKE_KEY="smoke-key" \
    "$PRISM_REPO/target/debug/prism" >"$ROOT/prism.log" 2>&1 &
PIDS="$PIDS $!"
wait_for "$PRISM_PORT" "prism gateway" || exit 1

# THE NODE (gravity ADR-0001, effector ADR-0003): a real `effector` dials the venue's
# `/effector/<alias>` and becomes CALLABLE — the mounts are minted from the vocabulary crossed
# with the alias, so nothing is configured in a manifest for this machine. `open` posture means
# the network is the boundary, and the venue's log says the types came from the dial.
# The node's authority is the TOKEN it dials with (effector ADR-0003): mint one on the venue,
# which answers the readable form exactly once — hand it to the node and forget it.
curl -sf -X POST "http://127.0.0.1:$PRISM_PORT/admin/nodes" \
    -H 'content-type: application/json' -d '{"alias":"smoke-node"}' >"$ROOT/node.json" \
    || fail "minting a node token failed"
if [ "${SMOKE_PY:-0}" = 1 ]; then NODE_CARRIERS='["exec","induction"]'; else NODE_CARRIERS='["exec"]'; fi
NODE_TOKEN=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$ROOT/node.json") \
    || fail "the mint answered no token: $(cat "$ROOT/node.json")"

cat > "$ROOT/effector.json" <<JSON
{
  "class": "host",
  "control_plane_url": "ws://127.0.0.1:$PRISM_PORT/effector/smoke-node",
  "token": "$NODE_TOKEN",
  "capabilities": { "node_alias": "smoke-node", "carriers": $NODE_CARRIERS },
  "lease": {}
}
JSON
"$EFFECTOR_REPO/target/debug/effector" "$ROOT/effector.json" >"$ROOT/effector.log" 2>&1 &
NODE_PID=$!
PIDS="$PIDS $NODE_PID"
tries=0
until curl -s "http://127.0.0.1:$PRISM_PORT/admin/mounts" | grep -q '"node_connected":true'; do
    tries=$((tries + 1))
    [ "$tries" -gt 80 ] && fail "the node never became callable: $(curl -s "http://127.0.0.1:$PRISM_PORT/admin/mounts"; tail -5 "$ROOT/effector.log")"
    sleep 0.25
done
echo "ok: a node dialled in, and the venue publishes its mount"

# THE DEPLOYMENT IS CONFIGURED THROUGH ITS OWN DOOR, not through prism: `ui:gravity-model.save` is
# the application's event (gravity ADR-0007 §4), gated by the auth declaration prism reads. A fresh
# venue has no settings row, and a turn on one refuses BY NAME — so this step is what makes the
# CLI's turn possible at all, and a bad row here fails loudly instead of leaving a plausible error.
cat > "$ROOT/configure.py" <<PY
import asyncio
import json
import sys

import websockets

PORT, MEM, SSE, PASSWORD = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
ROWS = [
    {
        "collection": "models",
        "id": 1,
        "doc": {
            "alias": "default",
            "endpoint": f"{SSE}/v1",
            "model": "smoke-model",
            "key_env": "SMOKE_KEY",
            "weight": 1,
            "enabled": 1,
        },
    },
    {
        "collection": "settings",
        "id": 1,
        "doc": {
            "store_url": MEM,
            "memory_scope": 37,
            "memory_format": "plaintext",
            "default_model": 1,
        },
    },
]


async def frame(ws):
    return json.loads(await asyncio.wait_for(ws.recv(), 15))


async def main():
    async with websockets.connect(f"ws://127.0.0.1:{PORT}/ws?protocol=json") as ws:
        first = await frame(ws)
        if first.get("ev") != "connected":
            raise SystemExit(f"FAIL: the venue did not greet: {first}")
        await ws.send(json.dumps({"ev": "login", "args": {"login_name": "smoke", "password": PASSWORD}}))
        answer = await frame(ws)
        if answer.get("ev") != "login.result":
            await ws.send(
                json.dumps({"ev": "signup", "args": {"login_name": "smoke", "password": PASSWORD}})
            )
            answer = await frame(ws)
            if answer.get("ev") != "signup.result":
                raise SystemExit(f"FAIL: neither login nor signup was accepted: {answer}")
        # (The SCREEN is not asked for here: a booth's frames travel as an ACTION LIST, which
        # this leg deliberately does not hand to programs — the live check for the screen is the
        # browser probe, and the booth test holds the shape.)

        for row in ROWS:
            await ws.send(json.dumps({"ev": "ui:gravity-model.save", "args": row}))
        seen = []
        while len(seen) < len(ROWS):
            answer = await frame(ws)
            name = answer.get("ev")
            if name == "gravity-model.error":
                raise SystemExit(f"FAIL: the deployment refused a row: {answer}")
            if name == "gravity-model.saved":
                seen.append(answer["args"])
        print("ok: the deployment is configured through its own door:", seen)


asyncio.run(main())
PY

python3 "$ROOT/configure.py" "$PRISM_PORT" "http://127.0.0.1:$MEM_PORT" "http://127.0.0.1:$SSE_PORT" "smoke-pw" \
    >"$ROOT/configure.out" 2>&1 || fail "configuring the deployment failed: $(cat "$ROOT/configure.out")"
cat "$ROOT/configure.out"

cat > "$ROOT/gravity.json" <<JSON
{ "venue": { "url": "ws://127.0.0.1:$PRISM_PORT", "username": "smoke", "password_env": "SMOKE_PW" } }
JSON

# This turn is PINNED to channel 1: the legs below read that channel by name, and what this one
# proves is the round trip, not the addressing. The addressing has its own leg further down
# (`--tag` with no `--session`, where the channel has to follow the speaker).
GRAVITY_CONFIG="$ROOT/gravity.json" SMOKE_PW="smoke-pw" \
    "$GRAVITY_REPO/target/debug/gravity" --json --tag operator --session 1 "hi" \
    >"$ROOT/cli.out" 2>"$ROOT/cli.err"
code=$?
echo "cli exit: $code"
cat "$ROOT/cli.err"
[ "$code" -eq 0 ] || fail "the CLI exited $code"

python3 - "$ROOT/cli.out" "$EXPECTED" <<'PY' || exit 1
import json
import sys

out, expected = sys.argv[1], sys.argv[2]
lines = [line for line in open(out).read().splitlines() if line.strip()]
if len(lines) != 1:
    raise SystemExit(f"FAIL: expected one JSON line, got {len(lines)}: {lines}")
value = json.loads(lines[0])
if value.get("reply") != expected:
    raise SystemExit(f"FAIL: reply {value.get('reply')!r} != {expected!r}")
if value.get("session") != "1":
    raise SystemExit(f"FAIL: session {value.get('session')!r} != '1'")
if value.get("tag") != "operator":
    raise SystemExit(f"FAIL: the answer does not say who asked: {value}")
print(f"ok: reply {value['reply']!r} on session {value['session']}, as `{value['tag']}`")
PY

# WHICH MEMBER: k10r's member ids are uuids (identity, not a counter), so `1` is not one.
# Ask the channel who is in it rather than pinning the seed's id — the default member is a
# constant today, and a read-back that hardcodes it would follow the constant's change silently.
MEMBER=$(curl -s "http://127.0.0.1:$MEM_PORT/channels/1/members" \
    | python3 -c 'import json,sys; rows=json.load(sys.stdin); print(rows[0]["member_id"])' \
    || fail "k10r members read failed")
# The member this read returns is the channel's DEFAULT one (the single-party shape k10r seeds),
# not the caller: the caller is an agent, and it arrives by tag.
# The status AND the body: a bare `curl -sf` turns "404 with k10r's own words" into
# "k10r session read failed", which is how this step hid its reason the first time.
code=$(curl -s -o "$ROOT/session.json" -w '%{http_code}' \
    "http://127.0.0.1:$MEM_PORT/channels/1/members/$MEMBER/session")
[ "$code" = "200" ] || fail "k10r session read failed ($code): $(cat "$ROOT/session.json")"
python3 - "$ROOT/session.json" "$EXPECTED" <<'PY' || exit 1
import json
import sys

session = json.load(open(sys.argv[1]))
expected = sys.argv[2]
contents = [row["content"] for row in session["increment"]]
# The turn's NET EFFECT, not its transcript: the ask, one row per round that called a tool, and
# the answer (k10r ADR-0024). The number of rounds is the model's business, so the assertion is
# the SHAPE: the ask first, the answer last, and nothing between them that is not a call.
if not contents or contents[0] != "hi":
    raise SystemExit(f"FAIL: the session does not open with the ask: {contents!r}")
if contents[-1] != expected:
    raise SystemExit(f"FAIL: the session does not close with the answer: {contents!r}")
stray = [row for row in contents[1:-1] if not row.startswith("[tool] ")]
if stray:
    raise SystemExit(f"FAIL: a row that is neither a call nor the answer: {stray!r}")
print("ok: k10r recorded the turn:", len(contents), "rows,", len(contents) - 2, "of them calls")
PY

# The TOOL path, through the whole stack: the stub asked for `commit_memory`
# on its first inference round, so the fact must be in k10r's graph now. A
# reply alone proves nothing — the model can answer without ever reaching a
# tool — which is why this reads the graph instead of the transcript.
code=$(curl -s -o "$ROOT/graph.json" -w '%{http_code}' "http://127.0.0.1:$MEM_PORT/graph/37")
[ "$code" = "200" ] || fail "k10r graph read failed ($code): $(cat "$ROOT/graph.json")"
# THE RENDERER LEG (prism ADR-0017 §10, gravity ADR-0001): a page is a `/channel` client that
# speaks `Outflow {dest, data}` and is answered with action lists. `SMOKE_PAGE=1` drives ONE
# connection through two turns and counts what each turn draws — the measurement gravity's PLAN
# asks for ("a duplicated patch frame ... measure in a clean venue with one tab"): with one tab, a
# frame that arrives twice is the venue's, not a second window's.
if [ "${SMOKE_PAGE:-0}" = "1" ]; then
    cat > "$ROOT/page.py" <<'PAGE'
import asyncio
import collections
import json
import sys

import websockets

PORT, TURNS = sys.argv[1], int(sys.argv[2])


def text_of(said):
    """The wire carries JSON text; a binary frame is the same bytes (the venue transcodes the
    booth's CBOR into the client's codec and may hand it back as binary)."""
    return said.decode() if isinstance(said, (bytes, bytearray)) else said


async def settle(ws, ops, budget=25.0):
    """Read until the turn says it is over, collecting every action list."""
    loop = asyncio.get_event_loop()
    while budget > 0:
        started = loop.time()
        try:
            said = await asyncio.wait_for(ws.recv(), timeout=min(budget, 5.0))
        except asyncio.TimeoutError:
            budget -= loop.time() - started
            continue
        budget -= loop.time() - started
        for op in json.loads(text_of(said)):
            ops.append(op)
            slot = op.get("slot") or op.get("name") or ""
            # The end is a fact, not a quiet socket. An ENDING IN ERROR IS A FAILURE of this leg:
            # a page that can submit but cannot complete a turn looks the same from here otherwise.
            if slot.endswith("turn.error"):
                print(f"  turn.error: {json.dumps(op)[:400]}")
                return False
            if slot.endswith("turn.done"):
                return True
    return False


async def main():
    async with websockets.connect(f"ws://127.0.0.1:{PORT}/channel") as ws:
        # The SAME account the CLI used: one deployment serves one person, and the member
        # identity a turn runs as comes from the venue's `sender`, not from the page.
        await ws.send(
            json.dumps(
                {"dest": "login", "data": {"login_name": "smoke", "password": "smoke-pw"}}
            )
        )
        # The first facts of a connection: token, connect. A page reads them and starts talking.
        for _ in range(6):
            said = text_of(await ws.recv())
            if "token" in said or "connect" in said:
                break
        for turn in range(TURNS):
            ops = []
            await ws.send(
                json.dumps(
                    {
                        # BARE, no `ui:` marker: a page speaks the stripped vocabulary on both
                        # sides (prism adds the marker on the way in, strips it on the way out).
                        # Sending the prefixed name makes the venue read it as a view event to
                        # paint, which "reaches no renderer" and is otherwise silent.
                        "dest": "turn.submit",
                        # A PAGE NAMES NOTHING BUT THE TEXT (gravity ADR-0007 §2, prism
                        # ADR-0018): the venue stamps who is speaking, and the channel is the
                        # member's own — the deployment serves one person. Handing ids here would
                        # hide a page-leg break behind a client that knows too much.
                        "data": {"user_message": "hi", "event": "ui:turn.delta"},
                    }
                )
            )
            ended = await settle(ws, ops)
            by_slot = collections.Counter(
                op.get("slot") or op.get("name") for op in ops
            )
            ids = [
                op.get("data", {}).get("id")
                for op in ops
                if isinstance(op.get("data"), dict)
            ]
            dupes = [i for i, n in collections.Counter(ids).items() if i and n > 1]
            print(f"turn {turn + 1}: {len(ops)} actions, ended={ended}, slots={dict(by_slot)}")
            if not ended:
                raise SystemExit(f"FAIL: turn {turn + 1} did not finish on the renderer leg")
            if dupes:
                print(f"  DUPLICATED ids: {dupes}")


asyncio.run(main())
PAGE
    echo "--- the renderer leg: one tab, two turns ---"
    python3 "$ROOT/page.py" "$PRISM_PORT" 2 || fail "the page leg failed"
fi

# RESIDENCY IS THE CARRIER'S (gravity ADR-0014 §5): this mount's runner is the one-shot
# `exec` form — one process per call, nothing retained (runner_nu.nu says so in its own
# header), so the node must hold NO child once the calls are done. A resident child here
# would mean the declaration and the wire disagree.
runners=$(pgrep -P "$NODE_PID" | wc -l)
if [ "${SMOKE_PY:-0}" = 1 ]; then
    # THE RESIDENT LEG, seen from the outside: the guest is a live child of the node, and it is
    # still there after every call — one process serving many calls is the whole difference
    # (gravity ADR-0014 §5). The one-shot `nu` mount adds nothing to this count.
    # ONE GUEST PER CONVERSATION, and no more: the key is `{mount}/{session}` and a session is
    # `channel-<id>-member-<id>`, so the CLI's turn is one conversation and the page's TWO turns
    # are another (they share a channel and a member). A per-turn key would leave 3 here, and a
    # per-call key 5 — this number is what says which one the node used.
    if [ "${SMOKE_PAGE:-0}" = 1 ]; then expected=2; else expected=1; fi
    guests=$(pgrep -aP "$NODE_PID" | grep -c 'runner_py_guest')
    if [ "$guests" -eq "$expected" ] && [ "$runners" -eq "$expected" ]; then
        echo "ok: $guests resident guest(s) for $expected conversation(s) — one process served every call in each"
    else
        pgrep -aP "$NODE_PID" | sed 's/^/  /'
        fail "resident leg: expected $expected live guest(s), found $runners child(ren), $guests of them guests"
    fi
else
    if [ "$runners" -eq 0 ]; then
        echo "ok: the one-shot runner left no child behind (exec: one process per call)"
    else
        pgrep -aP "$NODE_PID" | sed 's/^/  /'
        fail "the mount declares the one-shot carrier, but the node holds $runners child(ren)"
    fi
fi

python3 - "$ROOT/graph.json" "$ROOT/code_id" <<'PY' || exit 1
import json
import sys

graph = json.load(open(sys.argv[1]))
code_id = open(sys.argv[2]).read().strip()
names = sorted(node["name"] for node in graph["nodes"])
# The code row becomes a graph node when a RUN reaches it (the attempt/success touches write it),
# which is the difference between a row that exists and a script that ran on a machine.
if f"code-{code_id}" not in names:
    raise SystemExit(f"FAIL: the code row never ran on the node; graph nodes: {names}")
if "alice" not in names or "tea" not in names:
    raise SystemExit(f"FAIL: the tool's write never landed; graph nodes: {names}")
edges = [(link["kind"]) for link in graph["links"]]
if "likes" not in edges:
    raise SystemExit(f"FAIL: the fact's edge is missing; link kinds: {edges}")
print("ok: the node ran the code row and commit_memory landed:", names, edges)
PY

# WHO SPOKE. Every row names its speaker: the caller's words carry the identity its TAG derives,
# and everything the turn said carries the agent's (gravity AGENT_MEMBER, registered so the bracket
# reads "gravity" instead of a uuid). Without this a channel with two speakers reads as a monologue
# — which is what it did until the rows learned to say who they belong to (k10r ADR-0009 §1).
#
# The caller is an AGENT, never the person who logged in: prism stamps the account on the envelope,
# and a declared identity that k10r knows as an agent outranks it (ADR-0015 §1). So the speakers in
# this channel are exactly two — the tag's agent and gravity — and the venue's stamp appearing as a
# speaker would mean the stamp won, which is the thing this leg is here to catch.
curl -s "http://127.0.0.1:$MEM_PORT/channels/1/messages" >"$ROOT/rows.json"
python3 - "$ROOT/rows.json" 01900000-0000-7000-8000-000000000001 operator \
    00000000-0000-0000-0000-000000000001 <<'PY' || exit 1
import json
import sys

rows = json.load(open(sys.argv[1]))
agent, tag, seeded = sys.argv[2], sys.argv[3], sys.argv[4]
if not rows:
    raise SystemExit("FAIL: the channel has no rows to attribute")
blank = [row for row in rows if not row.get("sender")]
if blank:
    raise SystemExit(f"FAIL: {len(blank)} row(s) name no speaker: {blank[0]}")
senders = {row["sender"]: row["sender_name"] for row in rows}
if senders.get(agent) != "gravity":
    raise SystemExit(f"FAIL: the agent is not the registered speaker it should be: {senders}")
callers = [who for who in senders if who != agent]
if len(callers) != 1:
    raise SystemExit(f"FAIL: expected ONE caller in the channel, found {senders}")
if senders[callers[0]] != tag:
    raise SystemExit(f"FAIL: the caller is not the agent its tag names: {senders}")
if callers[0] == seeded:
    raise SystemExit(f"FAIL: the rows carry the seeded member: {senders}")
print(f"ok: every row names its speaker — {tag} (the tag's agent) and {senders[agent]}")
PY

# THE DIRECTORY, through the real stack: the agent was registered by a turn, so it must be
# findable by name — that is the read an agent makes before adding somebody to a channel.
curl -s "http://127.0.0.1:$MEM_PORT/members?name=gravity" >"$ROOT/dir.json"
python3 - "$ROOT/dir.json" <<'PY' || exit 1
import json
import sys

found = json.load(open(sys.argv[1]))
if [row["display_name"] for row in found] != ["gravity"]:
    raise SystemExit(f"FAIL: the directory does not answer the agent by name: {found}")
if found[0]["role"] != 1:
    raise SystemExit(f"FAIL: the agent is not tagged AGENT (role 1): {found}")
print("ok: the directory finds the agent by name and tags it AGENT")
PY

# THE TAG IS AN AGENT OF ITS OWN. `--tag` makes the CLI an agent named by that tag, derived
# from the name — so the SAME name is the same speaker next run, which is how one-turn invocations
# accumulate into one conversation in the channel. The venue still knows who logged in, and the
# store does not honour a declared identity that is a PERSON, so this is a process speaking for
# itself rather than a person pretending to be someone else. WHO supervises is the agent's call
# (`find_member` + `supervise`), not the client's, so the client carries no such flag.
GRAVITY_CONFIG="$ROOT/gravity.json" SMOKE_PW="smoke-pw" \
    "$GRAVITY_REPO/target/debug/gravity" --json --session 7 \
    --tag "drift check" "hi" >"$ROOT/cli-task.out" 2>"$ROOT/cli-task.err"
code=$?
[ "$code" -eq 0 ] || fail "the tagged run exited $code: $(cat "$ROOT/cli-task.err")"
curl -s "http://127.0.0.1:$MEM_PORT/channels/7/messages" >"$ROOT/task-rows.json"
curl -s "http://127.0.0.1:$MEM_PORT/channels/7/members" >"$ROOT/task-members.json"
curl -s "http://127.0.0.1:$MEM_PORT/members?name=drift%20check" >"$ROOT/task-dir.json"
python3 - "$ROOT/task-rows.json" "$ROOT/task-members.json" "$ROOT/task-dir.json" <<'PY' || exit 1
import json
import sys

rows = json.load(open(sys.argv[1]))
members = {m["member_id"]: m for m in json.load(open(sys.argv[2]))}
directory = json.load(open(sys.argv[3]))

context = [row for row in directory if row["display_name"] == "drift check"]
if len(context) != 1:
    raise SystemExit(f"FAIL: the context is not one agent in the directory: {directory}")
context_id = context[0]["member_id"]
if context[0]["role"] != 1:
    raise SystemExit(f"FAIL: the context speaks as a person, not an agent: {context[0]}")
asked = [row for row in rows if row["role"] == "user"]
if not asked or any(row["sender"] != context_id for row in asked):
    raise SystemExit(f"FAIL: the asks do not belong to the context agent: {asked}")
if context_id not in members:
    raise SystemExit(f"FAIL: the context agent is not a member of the channel it speaks in: {list(members)}")
print(f"ok: the context speaks as its own agent ({context_id[:8]}…), in channel 7")
PY

# THE CHANNEL FOLLOWS THE SPEAKER. No `--session` anywhere in this call: k10r records which channel
# is a speaker's own (`solo`), the booth asks for it by name, and a context that has never run gets
# its line opened on the spot. A person's first message takes the same path — which is why the CLI
# no longer defaults to channel 1: that default was the client guessing at an identity the store is
# the one that knows.
GRAVITY_CONFIG="$ROOT/gravity.json" SMOKE_PW="smoke-pw" \
    "$GRAVITY_REPO/target/debug/gravity" --json \
    --tag "unpinned line" "hi" >"$ROOT/cli-own.out" 2>"$ROOT/cli-own.err"
code=$?
[ "$code" -eq 0 ] || fail "the unpinned run exited $code: $(cat "$ROOT/cli-own.err")"
curl -s "http://127.0.0.1:$MEM_PORT/members?name=unpinned%20line" >"$ROOT/own-dir.json"
python3 - "$ROOT/own-dir.json" "$MEM_PORT" <<'PY' || exit 1
import json
import sys
import urllib.request

directory = json.load(open(sys.argv[1]))
port = sys.argv[2]
rows = [m for m in directory if m["display_name"] == "unpinned line"]
if len(rows) != 1:
    raise SystemExit(f"FAIL: the unpinned context is not one agent: {directory}")
member = rows[0]["member_id"]
lines = json.load(
    urllib.request.urlopen(f"http://127.0.0.1:{port}/members/{member}/channels?solo=1")
)
if len(lines) != 1:
    raise SystemExit(f"FAIL: expected ONE own line for the context, got {lines}")
channel = lines[0]["channel_id"]
if channel in (1, 7):
    raise SystemExit(
        f"FAIL: the unpinned context spoke in somebody else's channel ({channel})"
    )
said = json.load(
    urllib.request.urlopen(f"http://127.0.0.1:{port}/channels/{channel}/messages")
)
if not said:
    raise SystemExit("FAIL: the context's own line is empty")
if said[0]["sender"] != member or said[0]["role"] != "user":
    who = json.load(
        urllib.request.urlopen(f"http://127.0.0.1:{port}/channels/{channel}/members")
    )
    raise SystemExit(
        f"FAIL: the question is not the context's own: {said[0]}"
        f"\n  channel {channel} holds {who}"
        f"\n  lines of the context: {lines}"
    )
if said[-1]["role"] != "assistant":
    raise SystemExit(f"FAIL: the line does not end with the answer: {said[-1]}")
print(f"ok: an unpinned context opened its own line ({channel}) — no id typed anywhere")
PY

echo "PASS (logs under $ROOT)"
