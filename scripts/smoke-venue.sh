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
#   4. `gravity --json hi`       — the CLI, as a user would run it
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
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1])
CHUNKS = ["hello", " world"]


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_POST(self):
        self.rfile.read(int(self.headers.get("content-length", 0)))
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("cache-control", "no-cache")
        self.send_header("transfer-encoding", "chunked")
        self.end_headers()

        def chunk(payload: bytes) -> None:
            self.wfile.write(b"%x\r\n%s\r\n" % (len(payload), payload))
            self.wfile.flush()

        for piece in CHUNKS:
            event = {"choices": [{"index": 0, "delta": {"content": piece}}]}
            chunk(b"data: " + json.dumps(event).encode() + b"\n\n")
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

PRISM_ADDR="127.0.0.1:$PRISM_PORT" PRISM_IDENTITY=open PRISM_DATA="$ROOT/prism-data" \
GRAVITY_PROVIDER_BIN="$GRAVITY_REPO/target/debug/gravity-provider" \
GRAVITY_TURN_BIN="$GRAVITY_REPO/target/debug/gravity-turn" \
GRAVITY_STORE_URL="http://127.0.0.1:$MEM_PORT" \
GRAVITY_ENDPOINT="http://127.0.0.1:$SSE_PORT/v1" \
GRAVITY_MODEL="smoke-model" \
GRAVITY_KEY_ENV="SMOKE_KEY" \
SMOKE_KEY="smoke-key" \
    "$PRISM_REPO/target/debug/prism" >"$ROOT/prism.log" 2>&1 &
PIDS="$PIDS $!"
wait_for "$PRISM_PORT" "prism gateway" || exit 1

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

echo "PASS (logs under $ROOT)"