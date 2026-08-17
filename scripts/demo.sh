#!/usr/bin/env bash
#
# freshdag end-to-end demo.
#
# Drives a synthetic Claude Code session through the hook binary, marks
# the artifact it produced, then asks `freshdag check` about it twice —
# once with the input untouched, once after changing it.
#
# Nothing here is mocked: the same two binaries a real install uses, a
# real store on disk, and the real `file://` probe.
#
# Usage:  scripts/demo.sh [workdir]

set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="${1:-$(mktemp -d)}"
STORE="$WORK/.freshdag"
SRC="$WORK/sales.csv"
OUT="$WORK/report.json"

cargo build --quiet -p freshdag-cli -p freshdag-adapter-claude
HOOK="$REPO/target/debug/freshdag-claude-hook"
FRESHDAG="$REPO/target/debug/freshdag"

mkdir -p "$WORK"
printf 'region,revenue\nEMEA,120\nAMER,340\n' > "$SRC"

SESSION="1e9c4d2a-3f5b-4a7c-9d81-6b2e0f4a7c31"
# The Write tool's `content`, escaped for embedding in the hook payload's
# JSON. The literal bytes written to disk are the unescaped form below.
CONTENT_JSON='{\"total\":460}\n'

hook() { "$HOOK" --store "$STORE"; }

echo "== 1. the agent works, and the hook records what it touched =="

printf '{"session_id":"%s","cwd":"%s","hook_event_name":"SessionStart","source":"startup"}' \
  "$SESSION" "$WORK" | hook

printf '{"session_id":"%s","cwd":"%s","hook_event_name":"PreToolUse","tool_name":"Read","tool_input":{"file_path":"%s"}}' \
  "$SESSION" "$WORK" "$SRC" | hook

printf '{"session_id":"%s","cwd":"%s","hook_event_name":"PreToolUse","tool_name":"Write","tool_input":{"file_path":"%s","content":"%s"}}' \
  "$SESSION" "$WORK" "$OUT" "$CONTENT_JSON" | hook

# The agent actually writes the file the Write tool described.
printf '{"total":460}\n' > "$OUT"

echo "   $(wc -l < "$STORE/events.jsonl" | tr -d ' ') IR events recorded"
echo "   $(wc -l < "$STORE/coverage.jsonl" | tr -d ' ') coverage manifest(s) published"
echo

echo "== 2. declare which file was the artifact =="
"$FRESHDAG" mark --store "$STORE" "$OUT"
echo

echo "== 3. check it, with nothing changed =="
set +e
"$FRESHDAG" check --store "$STORE" "$OUT"
echo "   exit $?"
set -e
echo

echo "== 4. someone edits the input the report was built from =="
printf 'region,revenue\nEMEA,120\nAMER,999\n' > "$SRC"
set +e
"$FRESHDAG" check --store "$STORE" "$OUT" | sed -n '1,14p'
echo "   exit ${PIPESTATUS[0]}"
set -e
echo

echo "== 5. and marking a file nobody recorded producing is refused =="
echo 'unrelated' > "$WORK/stranger.txt"
set +e
"$FRESHDAG" mark --store "$STORE" "$WORK/stranger.txt"
echo "   exit $?"
set -e

echo
echo "store: $STORE"
