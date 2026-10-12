#!/usr/bin/env bash
# End-to-end smoke test: real binary, real daemon, real socket, mock model.
# Runs in an isolated config/data directory so it never touches ~/.config.
#
#   scripts/e2e_mock.sh [path-to-aether-binary]
set -euo pipefail
cd "$(dirname "$0")/.."
. scripts/lib.sh
BIN=$(resolve_path "${1:-target/debug/aether}")
ROOT=$(mktemp -d "$(tmp_base)/aether-e2e-XXXXXX")
export AETHER_CONFIG_DIR="$ROOT/config" AETHER_DATA_DIR="$ROOT/data" AETHER_SOCKET="$ROOT/aetherd.sock"
mkdir -p "$AETHER_CONFIG_DIR" "$AETHER_DATA_DIR" "$ROOT/workspace"
echo "# sample" > "$ROOT/workspace/README.md"
# Ask the kernel for two distinct unused ports instead of choosing from small
# ranges. Holding the first socket while asking for the second guarantees the
# mock and daemon do not receive the same port. The sockets must be released
# before those programs bind, so another process can still claim either port
# in that short interval.
PORTS=$(python3 -c '
import socket
sockets = []
try:
    for _ in range(2):
        sock = socket.socket()
        sock.bind(("127.0.0.1", 0))
        sockets.append(sock)
    print(*(sock.getsockname()[1] for sock in sockets))
finally:
    for sock in sockets:
        sock.close()
')
PORT=${PORTS%% *}
# The daemon's own TCP transport, for the generated client to reach it on.
LISTEN_PORT=${PORTS#* }
export AETHER_ADDRESS="127.0.0.1:$LISTEN_PORT"
# Generated, not literal: this file is the most likely place a real config
# gets copied from, and a token in it would travel with the copy.
AETHER_TOKEN=$(openssl rand -hex 32 2>/dev/null || python3 -c \
  "import secrets; print(secrets.token_hex(32))")
export AETHER_TOKEN
cat > "$AETHER_CONFIG_DIR/providers.toml" <<EOF
[providers.mock]
name = "Mock (local)"
protocol = "openai-chat-completions"
base_url = "http://127.0.0.1:$PORT/v1"
[providers.mock.auth]
none = true
[[providers.mock.models]]
id = "mock-1"
name = "Mock model"
[providers.mock.models.capabilities]
context_window = 32768
max_output_tokens = 4096
tools = true
[[providers.mock.models]]
id = "mock-vision"
name = "Mock model that takes images"
[providers.mock.models.capabilities]
context_window = 32768
max_output_tokens = 4096
tools = true
vision = true

# The same mock server over the Responses wire, so both adapters are
# smoke-tested against one script. family picks the GPT model profile.
[providers.mock-responses]
name = "Mock responses (local)"
protocol = "openai-responses"
base_url = "http://127.0.0.1:$PORT/v1"
family = "gpt"
[providers.mock-responses.auth]
none = true
[[providers.mock-responses.models]]
id = "mock-1"
name = "Mock model"
[providers.mock-responses.models.capabilities]
context_window = 32768
max_output_tokens = 4096
tools = true
vision = true
EOF
{
  printf '[model]\ndefault = "mock/mock-1"\n'
  printf '\n[listen]\naddress = "%s"\ntoken_env = "AETHER_TOKEN"\n' "$AETHER_ADDRESS"
} > "$AETHER_CONFIG_DIR/config.toml"

python3 scripts/mock_openai.py "$PORT" > "$ROOT/mock.log" 2>&1 &
MOCK=$!
cleanup() {
  if [ -n "${DAEMON:-}" ]; then stop_test_child "$DAEMON"; fi
  kill "$MOCK" 2>/dev/null || true
  wait "$MOCK" 2>/dev/null || true
  echo "artifacts in $ROOT"
}
trap cleanup EXIT

# Fail at startup with the mock server's diagnostic instead of letting the
# first provider request time out much later in the end-to-end run.
mock_ready=0
for ((attempt = 0; attempt < 50; attempt++)); do
  if grep -q "mock openai server on http://127.0.0.1:$PORT/v1" "$ROOT/mock.log"; then
    mock_ready=1
    break
  fi
  if ! kill -0 "$MOCK" 2>/dev/null; then
    break
  fi
  sleep 0.1
done
if [ "$mock_ready" -ne 1 ]; then
  echo "mock OpenAI server failed to start" >&2
  cat "$ROOT/mock.log" >&2
  exit 1
fi

step() { printf '\n\033[1;34m==> %s\033[0m\n' "$*"; }
start_test_daemon() {
  ( cd "$ROOT/workspace" && exec "$BIN" daemon --foreground ) > "$ROOT/daemon.log" 2>&1 &
  DAEMON=$!
  wait_for_test_daemon "$BIN" "$DAEMON" "$ROOT/daemon.log" 60
}
step "daemon"
start_test_daemon
"$BIN" status | tee "$ROOT/status.log"
# No [scheduler] cap or budget is configured here, and the mock provider is
# not an engine, so the limits line must show the uncapped/no-budget defaults
# whatever the live agent counts say.
grep -qE '^limits +[0-9]+ turns global · engines 0 running · uncapped · run budget none$' \
  "$ROOT/status.log" \
  || { echo "limits line missing or unexpected: $(cat "$ROOT/status.log")"; exit 1; }
step "models"
"$BIN" models
step "auth"
"$BIN" auth
step "providers"
"$BIN" providers
"$BIN" providers --json > /dev/null
step "task: create a taskless owner from the CLI"
"$BIN" new --name planner --model mock/mock-1 --json > "$ROOT/owner.json"
OWNER=$(python3 -c 'import json,sys; a=json.load(open(sys.argv[1])); assert a.get("current_task") is None; assert a["name"] == "planner"; print(a["id"])' "$ROOT/owner.json")
"$BIN" task --owner "$OWNER" "hello" --json > "$ROOT/owned-task.json"
python3 -c 'import json,sys; t=json.load(open(sys.argv[1])); assert t["owner"] == sys.argv[2]; assert t["objective"] == "hello"' "$ROOT/owned-task.json" "$OWNER"
"$BIN" task "pending unowned work" --json > "$ROOT/unowned-task.json"
python3 -c 'import json,sys; t=json.load(open(sys.argv[1])); assert t.get("owner") is None; assert t["status"]["state"] == "queued"' "$ROOT/unowned-task.json"
step "run: hello"
"$BIN" run "hello"
step "run: list files (tool call)"
"$BIN" run --yes "please list the files in the workspace"
step "run: openai-responses adapter, same mock server"
"$BIN" run --yes --model mock-responses/mock-1 "please list the files in the workspace" \
  2>&1 | tee "$ROOT/responses.log"
grep -q "bash" "$ROOT/responses.log" \
  || { echo "no tool call over the responses adapter"; exit 1; }

step "send --image: the model gets the picture, or the stored file's path"
python3 scripts/check_image_send.py "$AETHER_SOCKET" "$BIN" "$ROOT"

step "run: delegate"
"$BIN" run --yes "delegate the listing to a child" 2>&1 | tee "$ROOT/delegate.log"
grep -q "Delegated to agent" "$ROOT/delegate.log" \
  || { echo "delegation did not create a child"; exit 1; }
step "run: background shell process"
# One run, one agent: the job is started and then listed while it is still
# alive. Listing from a second `aether run` would be a different agent, so
# it could only ever say "none" - an assertion that cannot fail.
"$BIN" run --yes "start a background job" 2>&1 | tee "$ROOT/bg.log"
grep -q "in the background" "$ROOT/bg.log" || { echo "no background handle"; exit 1; }
grep -qE "proc_[0-9A-Z]+ running · " "$ROOT/bg.log" \
  || { echo "shell_processes did not list a running process"; exit 1; }
grep -q "tick 1" "$ROOT/bg.log" \
  || { echo "shell_output did not return the background process output"; exit 1; }
grep -qE "stopped proc_[0-9A-Z]+" "$ROOT/bg.log" \
  || { echo "shell_stop did not stop the background process"; exit 1; }
grep -q "no background processes" "$ROOT/bg.log" \
  || { echo "shell_processes still listed a process after it was stopped"; exit 1; }

step "artifacts: list and show"
"$BIN" run --yes "produce a big output" 2>&1 | tee "$ROOT/big.log" | tail -3
"$BIN" artifacts list --limit 5
ART=$("$BIN" artifacts list --limit 1 --json | python3 -c 'import json,sys; r=json.load(sys.stdin); print(r[0]["id"] if r else "")')
test -n "$ART" || { echo "no artifact was produced"; exit 1; }
"$BIN" artifacts show "$ART" --max-bytes 120 | head -10

step "approval rules: remember a decision, list it, revoke it"
RULE=$("$BIN" rules allow --tool bash --argument 'git status*' --everywhere --note "read-only git" --json \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')
"$BIN" rules list
DENY=$("$BIN" rules deny --tool bash --argument 'rm -rf *' --everywhere --note "never" --json \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')
"$BIN" rules revoke "$DENY"
"$BIN" rules revoke "$RULE"
test -z "$("$BIN" rules list --json | python3 -c 'import json,sys; print("".join(r["id"] for r in json.load(sys.stdin)))')" \
  || { echo "revoked rule still active"; exit 1; }
"$BIN" rules list --all
step "plain mode over a pipe"
# A closed pipe is the only way out a script has, so this must finish on its
# own and must have said what the screen would have said. The pipe is held
# open for a few seconds first, the way a person or a screen reader holds it.
# Every step above left a finished agent behind, so this also stands in for
# the store a script finds in the wild: plain mode must start an agent of its
# own rather than address one of them.
PLAIN_OUT="$ROOT/plain.txt"
fail_plain() {
  echo "plain mode: $1"
  grep -E '^- session agent: ' "$PLAIN_OUT" || echo "plain mode adopted no agent"
  "$BIN" agents --all || true
  cat "$PLAIN_OUT"
  exit 1
}
if ! ( printf '/usage\nhello\n'; sleep 5 ) | with_timeout 90 "$BIN" --plain > "$PLAIN_OUT" 2>&1; then
  fail_plain "did not exit on EOF (or failed)"
fi
cat "$PLAIN_OUT"
check_plain() {
  grep -qE "$1" "$PLAIN_OUT" || fail_plain "$2"
}
check_plain 'account usage' "never printed account usage"
check_plain 'do(es)? not report usage limits' "hid unavailable account usage"
check_plain '^> hello$' "never confirmed the message"
check_plain 'mock model' "never printed the reply"
check_plain 'fleet [1-9][0-9]* live' "reported no live agent"
check_plain '^- session agent: ' "never said which agent it addressed"
if grep -q 'is completed' "$PLAIN_OUT"; then
  fail_plain "addressed an agent that had already finished"
fi
if LC_ALL=C grep -q '[[:cntrl:]]' "$PLAIN_OUT"; then
  fail_plain "emitted a control character"
fi

step "compaction via the protocol client"
python3 - "$AETHER_SOCKET" <<'PY'
import sys
sys.path.insert(0, "evals")
from aether_client import AetherClient
c = AetherClient(sys.argv[1])
a = c.create_agent(role="chat", long_lived=True)["id"]
c.subscribe(a)
for i in range(7):
    c.send(a, f"hello again {i}")
    c.wait_for_reply(a)
c.compact(a)
c.send(a, "hello after compaction")
c.wait_for_reply(a)
ev = c.wait_for_event(a, "compaction.completed", timeout=10)
k = ev["kind"]
print(f"compaction.completed v{k['version']} folded events {k['from_seq']}..{k['to_seq']}")
mem = c.find_event(a, "memory.accepted")
print("memory accepted:", bool(mem))
snap = c.find_event(a, "snapshot.created")
print("last snapshot layers:", [(l["kind"], l["tokens"]) for l in snap["kind"]["layers"]])
PY

step "set: every knob form the CLI parses, on a live agent"
AGENT=$(python3 - "$AETHER_SOCKET" <<'PY'
import sys
sys.path.insert(0, "evals")
from aether_client import AetherClient
print(AetherClient(sys.argv[1]).create_agent(role="knobs", long_lived=True)["id"])
PY
)
"$BIN" set "$AGENT" --priority high --network denied --backend container:alpine:3 --deadline 2035-01-02T03:04:05Z
"$BIN" set "$AGENT" --deadline none --backend host --json | python3 -c '
import json, sys
a = json.load(sys.stdin)
assert a["config"].get("deadline") is None, a["config"]
assert a["scope"].get("backend", {}).get("backend", "host") == "host", a["scope"]
print("set: deadline cleared, backend back on the host")
'

step "pause / resume over a subtree"
python3 - "$AETHER_SOCKET" <<'PY'
import sys
sys.path.insert(0, "evals")
from aether_client import AetherClient
c = AetherClient(sys.argv[1])
root = c.create_agent(role="supervisor")["id"]
child = c.create_agent(role="worker", parent=root, effort="low")["id"]
paused = c.pause(root, subtree=True)
assert [a["id"] for a in paused["agents"]] == [root, child], paused
assert c.get_agent(child)["paused"] is True
# A child created under a held supervisor is held with it.
later = c.create_agent(role="worker", parent=child, effort="low")["id"]
assert c.get_agent(later)["paused"] is True, "a later child inherits the pause"
resumed = c.resume(root, subtree=True)
assert sorted(a["id"] for a in resumed) == sorted([root, child, later]), resumed
assert c.get_agent(later)["paused"] is False
# And one message reaches the whole subtree.
sent = c.send_subtree(root, "stand down")
assert len(sent["deliveries"]) == 3, sent
print("pause/resume --subtree and send --subtree cover", len(sent["deliveries"]), "agents")
PY

step "agents / tasks"
"$BIN" agents --all
"$BIN" tasks
step "events (tail)"
"$BIN" events | tail -25
step "stop"
"$BIN" stop
wait "$DAEMON"
DAEMON=
step "restart and verify state survived"
start_test_daemon
"$BIN" status
"$BIN" agents --all
step "generated python client over tcp"
python3 scripts/check_python_client.py
printf '\n\033[1;32me2e passed\033[0m\n'
