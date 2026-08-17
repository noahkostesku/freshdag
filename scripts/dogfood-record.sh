#!/usr/bin/env bash
#
# Snapshot a store into a `docs/DOGFOOD.md` entry.
#
# `BUILD_PLAN` W13 wants ten ordinary sessions recorded, "including the
# sessions where FreshDAG saw nothing useful". The recording should
# therefore be mechanical: an entry a human has to assemble by hand is an
# entry that gets skipped on the sessions least flattering to the tool,
# which are the ones that matter most.
#
# This prints a Markdown block to stdout. It does NOT append, and it does
# NOT edit `docs/DOGFOOD.md` — the prose about what you were doing is
# yours to write, and the log is worth nothing without it.
#
# Usage:
#   scripts/dogfood-record.sh [--store DIR] [--session N]
#
# What it will not do:
#   - round in the tool's favour
#   - infer anything the store does not record
#   - proceed on a store holding more than one session, without saying so

set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STORE="$REPO/.freshdag"
SESSION="N"

while [ $# -gt 0 ]; do
  case "$1" in
    --store)   STORE="$2"; shift 2 ;;
    --session) SESSION="$2"; shift 2 ;;
    -h|--help) sed -n '2,25p' "${BASH_SOURCE[0]}"; exit 0 ;;
    *) echo "unrecognized argument: $1" >&2; exit 2 ;;
  esac
done

if [ ! -f "$STORE/events.jsonl" ]; then
  echo "no FreshDAG store at $STORE" >&2
  exit 2
fi

cargo build --quiet -p freshdag-cli
FRESHDAG="$REPO/target/debug/freshdag"

"$FRESHDAG" coverage --store "$STORE" --json > /tmp/freshdag-dogfood.$$.json
trap 'rm -f /tmp/freshdag-dogfood.$$.json' EXIT

python3 - "$STORE" "$SESSION" /tmp/freshdag-dogfood.$$.json <<'PY'
import json, sys, collections, datetime

store, session, report_path = sys.argv[1], sys.argv[2], sys.argv[3]
r = json.load(open(report_path))

sessions, comps, tools = set(), set(), collections.Counter()
for line in open(f"{store}/events.jsonl"):
    if not line.strip():
        continue
    e = json.loads(line)
    sessions.add(e.get("session_id"))
    if e.get("computation_id"):
        comps.add(e["computation_id"])
    if e.get("kind") == "tool.invoked":
        tools[(e["payload"].get("tool_name"), e["payload"].get("tool_kind"))] += 1

now = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
total, blind = r["total_tool_calls"], r["blind_tool_calls"]
seen = total - blind
pct = f"{seen / total * 100:.0f}%" if total else "n/a"

print(f"## Session {session} — {now[:10]} — <what you were doing>\n")
print(f"**Status: {session} of 10.**\n")
print("### What the session was\n")
print("<Describe the work. Working style dominates the result, so say")
print("whether it was shell-heavy, edit-heavy, exploratory, or routine.>\n")
print("### Measured\n")
print(f"Snapshot at `{now}`, from `freshdag coverage --json`:\n")
print("| | |")
print("|---|---|")
print(f"| events | {r['events']} |")
print(f"| sessions in store | {len(sessions)} |")
print(f"| computations | {r['computations']} |")
print(f"| replay | {'deterministic' if r['deterministic'] else 'NOT REPRODUCIBLE'} |")
print(f"| tool calls | " + ", ".join(f"`{k}` {v}" for k, v in sorted(r["tool_calls"].items())) + " |")
print(f"| **could yield fs evidence** | **{seen} of {total} ({pct})** — upper bound |")
print(f"| obligations raised | {r['obligations']} |")
print(f"| obligations dischargeable | {'yes' if r['obligations_dischargeable'] else 'no'} |")
print(f"| dependency edges | {r['dependencies']} |")
print(f"| artifacts | {r['artifacts']} |")
if r["excluded"]:
    print("| excluded | " + ", ".join(f"{v} x {k}" for k, v in sorted(r["excluded"].items())) + " |")
print(f"| unregistered producers | {len(r['unregistered_producers'])} |")

print("\nTool names behind those kinds:\n")
for (name, kind), count in sorted(tools.items(), key=lambda x: -x[1]):
    print(f"- `{name}` (`{kind}`) x{count}")

if len(sessions) > 1:
    print(f"\n> **This store holds {len(sessions)} sessions.** The figures above are")
    print("> their sum, not one session. Either split the entry or say so.")

print("\n### What `check` said\n")
print("<Did you mark anything? What did `freshdag check` return, and was")
print("it right? 'Nothing was marked, and nothing prompted me to' is a")
print("finding — record it.>\n")
print("### Honest reading\n")
print("<What does this session tell you that the last one did not? If it")
print("tells you nothing, say that.>\n")
PY
