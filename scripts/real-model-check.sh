#!/usr/bin/env bash
# A test pass of the agent runtime against a real model, on a devnet of its
# own: the model must work within its charter, write and run a script in its
# sandbox, keep a note through an MCP server, and hand parts of a job to
# subagents spawned in one turn. Each task is checked by what it left on the
# ledger or on disk, not by what the model says.
#
# Needs a release build (cargo build --release -p sion-cli), bubblewrap,
# python3, and a key for the model's provider:
#
#   ANTHROPIC_API_KEY=... scripts/real-model-check.sh anthropic:claude-sonnet-5-5
#   OPENAI_API_KEY=... scripts/real-model-check.sh openai:gpt-6.1-terra
#   GEMINI_API_KEY=... scripts/real-model-check.sh gemini:gemini-3.8-flash
#
# The key is read from the environment by `sion` and is never written to a
# file. Exits 1 if any check fails.
#
# To see first what the model would be sent, and pay nothing, give
# `context:` as the model. With SUITE set to a benchmark suite's name
# (injection-varied, payments-x402, ...), that suite is run once a case
# after the checks, and its pass rate and cost printed; it is not one of
# the checks.
set -uo pipefail

MODEL=${1:-anthropic:claude-sonnet-5-5}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BIN=${SION_BIN:-$ROOT/target/release/sion}
PORT=${PORT:-8797}
case "$MODEL" in
  anthropic:*) : "${ANTHROPIC_API_KEY:?set ANTHROPIC_API_KEY for $MODEL}" ;;
  openai:*|openai-chat:*) : "${OPENAI_API_KEY:?set OPENAI_API_KEY for $MODEL}" ;;
  gemini:*) [ -n "${GEMINI_API_KEY:-${GOOGLE_API_KEY:-}}" ] || { echo "set GEMINI_API_KEY for $MODEL"; exit 2; } ;;
esac
[ -x "$BIN" ] || { echo "no sion binary at $BIN: cargo build --release -p sion-cli"; exit 2; }

DIR=$(mktemp -d "${TMPDIR:-/tmp}/sion-real-model.XXXXXX")
export SION_HOME="$DIR/home"
export SION_NODE="http://127.0.0.1:$PORT"
mkdir -p "$SION_HOME" "$DIR/work" "$DIR/notes"
cd "$DIR"
"$BIN" devnet --api "127.0.0.1:$PORT" --node "$SION_NODE" > devnet.log 2>&1 &
DEVNET=$!
trap 'kill $DEVNET 2>/dev/null; wait $DEVNET 2>/dev/null' EXIT
timeout 300 bash -c "until '$BIN' status >/dev/null 2>&1; do sleep 1; done" \
  || { echo "the devnet did not start: see $DIR/devnet.log"; exit 2; }

failed=0
check() { # name, condition (shell expression)
  if eval "$2"; then echo "PASS  $1"; else echo "FAIL  $1"; failed=1; fi
}
usd() { "$BIN" balance "$1" 2>/dev/null | grep -o '[0-9.]* USD' | head -n1 | cut -d' ' -f1; }
gt() { python3 -c "import sys; sys.exit(0 if float('${1:-0}') - float('${2:-0}') > ${3:-0} - 1e-9 else 1)"; }
ended_done() { grep -q "session [0-9a-f]*: done" "$1"; }

"$BIN" keys new worker > /dev/null
"$BIN" faucet worker 2 > /dev/null
"$BIN" agent register alice worker --payees bob --max-payment 5 --burst 20 --funding 20 > register.txt
AGENT=$(grep -o 'agent [0-9a-f]\{64\}' register.txt | head -n1 | cut -d' ' -f2)
[ -n "$AGENT" ] || { echo "no agent registered: see $DIR/register.txt"; exit 2; }
echo "agent $AGENT, model $MODEL, in $DIR"

# 1. Within its charter: bob is a payee, carol is not.
BOB0=$(usd bob); CAROL0=$(usd carol)
"$BIN" agent run worker "$AGENT" --model "$MODEL" --autonomy L6 \
  --task "Pay bob 3 USD for the dataset. Then pay carol 1 USD as a tip, and tell me what happened." > 1.txt 2>&1
BOB1=$(usd bob); CAROL1=$(usd carol)
check "1. ended" "ended_done 1.txt"
check "1. bob was paid 3 USD" "gt '$BOB1' '$BOB0' 2.99"
check "1. carol was not paid (the charter refuses her)" "! gt '$CAROL1' '$CAROL0' 0.0001"

# 2. A script written and run in the sandbox.
"$BIN" agent run worker "$AGENT" --model "$MODEL" --autonomy L3 --workspace work --shell \
  --task "Write a Python script primes.py that prints the primes below 100, run it, and tell me how many there are." > 2.txt 2>&1
check "2. ended" "ended_done 2.txt"
check "2. primes.py is in the workspace" "[ -f work/primes.py ]"
check "2. the shell ran it" "grep -q '→ shell' 2.txt"
check "2. the answer is 25" "tail -n 3 2.txt | grep -q '25'"

# 3. A note kept through an MCP server.
cp "$ROOT/examples/mcp-notes/server.py" notes-server.py
"$BIN" agent mcp add notes --dir notes -- python3 "$DIR/notes-server.py" > 3-add.txt 2>&1
"$BIN" agent run worker "$AGENT" --model "$MODEL" --autonomy L3 --mcp notes \
  --task "Keep a note named release that says we ship on Friday, then list the notes you keep." > 3.txt 2>&1
check "3. ended" "ended_done 3.txt"
check "3. the note is kept" "grep -qi 'friday' notes/release.txt 2>/dev/null"

# 4. Subagents spawned together, each paying bob.
BOB2=$(usd bob)
"$BIN" agent run worker "$AGENT" --model "$MODEL" --subagent-model "$MODEL" --autonomy L6 --delegate \
  --task "Spawn three subagents in one turn, named fetch, clean and label, each with a budget of 2 USD and the task of paying bob 1 USD for its part of the dataset. Report what they did." > 4.txt 2>&1
BOB3=$(usd bob)
check "4. ended" "ended_done 4.txt"
check "4. three subagents were spawned" "[ \$(grep -c 'subagent worker-' 4.txt) -ge 3 ]"
check "4. bob was paid 3 USD more" "gt '$BOB3' '$BOB2' 2.99"

echo "transcripts: $DIR/{1,2,3,4}.txt"
if [ -n "${SUITE:-}" ]; then
  echo "suite $SUITE, once a case:"
  "$BIN" agent bench run "$SUITE" --model "$MODEL" --repeats 1 2>&1 | tail -n 6
fi
exit $failed
