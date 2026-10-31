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
        with LOCK:
            round_index = ROUNDS
            ROUNDS += 1
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

# One skill declared before the turn: `resolve_skills` must have something to
# find, or the smoke proves the door exists and nothing else. Its step points at
# a code row the run never fetches — resolving is the call under test, the walk
# over the result is invoke_skill's (still unlisted, gravity Phase 4.6).
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
PRISM_ADDR="127.0.0.1:$PRISM_PORT" PRISM_IDENTITY=open PRISM_DATA="$ROOT/prism-data" \
GRAVITY_PROVIDER_BIN="$GRAVITY_REPO/target/debug/gravity-provider" \
GRAVITY_TURN_BIN="$GRAVITY_REPO/target/debug/gravity-turn" \
SMOKE_KEY="smoke-key" \
    "$PRISM_REPO/target/debug/prism" >"$ROOT/prism.log" 2>&1 &
PIDS="$PIDS $!"
wait_for "$PRISM_PORT" "prism gateway" || exit 1

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
        await ws.send(json.dumps({"ev": "login", "args": {"username": "smoke", "password": PASSWORD}}))
        answer = await frame(ws)
        if answer.get("ev") != "login.result":
            await ws.send(
                json.dumps({"ev": "signup", "args": {"username": "smoke", "password": PASSWORD}})
            )
            answer = await frame(ws)
            if answer.get("ev") != "signup.result":
                raise SystemExit(f"FAIL: neither login nor signup was accepted: {answer}")
        # THE SCREEN, over the real venue: an unconfigured deployment must still draw (empty forms
        # are what make it configurable), and the frames must come back under the stripped name.
        await ws.send(json.dumps({"ev": "ui:gravity-model", "args": {"slots": {"rack": "admin"}}}))
        answer = await frame(ws)
        if answer.get("ev") != "gravity-model":
            raise SystemExit(f"FAIL: the screen did not answer: {answer}")
        frames = answer["args"]["frames"]
        if [f["data"]["type"] for f in frames] != ["form", "form"]:
            raise SystemExit(f"FAIL: the screen drew {frames}")
        for drawn in frames:
            if drawn["data"]["bind"]["value"].get("dest") != "gravity-model.save":
                raise SystemExit(f"FAIL: a form submits the wrong event: {drawn}")
        print("ok: the screen draws two forms that submit gravity-model.save")

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

GRAVITY_CONFIG="$ROOT/gravity.json" SMOKE_PW="smoke-pw" \
    "$GRAVITY_REPO/target/debug/gravity" --json "hi" >"$ROOT/cli.out" 2>"$ROOT/cli.err"
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
print(f"ok: reply {value['reply']!r} on session {value['session']}")
PY

curl -sf "http://127.0.0.1:$MEM_PORT/channels/1/members/1/session" >"$ROOT/session.json" || fail "k10r session read failed"
python3 - "$ROOT/session.json" "$EXPECTED" <<'PY' || exit 1
import json
import sys

session = json.load(open(sys.argv[1]))
expected = sys.argv[2]
contents = [row["content"] for row in session["increment"]]
if contents != ["hi", expected]:
    raise SystemExit(f"FAIL: k10r holds {contents!r}, expected ['hi', {expected!r}]")
print("ok: k10r recorded the turn:", contents)
PY

# The TOOL path, through the whole stack: the stub asked for `commit_memory`
# on its first inference round, so the fact must be in k10r's graph now. A
# reply alone proves nothing — the model can answer without ever reaching a
# tool — which is why this reads the graph instead of the transcript.
curl -sf "http://127.0.0.1:$MEM_PORT/graph/37" >"$ROOT/graph.json" || fail "k10r graph read failed"
python3 - "$ROOT/graph.json" <<'PY' || exit 1
import json
import sys

graph = json.load(open(sys.argv[1]))
names = sorted(node["name"] for node in graph["nodes"])
if "alice" not in names or "tea" not in names:
    raise SystemExit(f"FAIL: the tool's write never landed; graph nodes: {names}")
edges = [(link["kind"]) for link in graph["links"]]
if "likes" not in edges:
    raise SystemExit(f"FAIL: the fact's edge is missing; link kinds: {edges}")
print("ok: commit_memory reached k10r through the venue:", names, edges)
PY

echo "PASS (logs under $ROOT)"