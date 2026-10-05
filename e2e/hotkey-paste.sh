#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$HOME/Applications/Yapr.app"
OUT="$ROOT/e2e/transcription-results/hotkey-paste.json"
SHOTS="$ROOT/e2e/screenshots"
SUPPORT="$(mktemp -d)"
mkdir -p "$SHOTS" "$(dirname "$OUT")"

finish() {
  osascript -e 'tell application "TextEdit" to close every document saving no' >/dev/null 2>&1 || true
  pkill -x Yapr 2>/dev/null || true
  rm -rf "$SUPPORT"
  open -g -a "$APP"
}
trap finish EXIT

cat >"$SUPPORT/settings.json" <<JSON
{
  "transcriptionModel": "microsoft/mai-transcribe-2",
  "cleanupModel": "google/gemini-3.5-flash-lite",
  "output": "paste"
}
JSON

pkill -x Yapr 2>/dev/null && sleep 0.5 || true
open -n -a "$APP" --env "YAPR_SUPPORT_DIR=$SUPPORT" --args --fixture "$ROOT/e2e/fixtures/pauses.wav"
sleep 2

osascript -e 'tell application "TextEdit" to activate' -e 'tell application "TextEdit" to make new document'
sleep 1

press_shortcut() {
  osascript -e 'tell application "System Events" to key code 49 using option down'
}

started=$(date +%s)
press_shortcut
sleep 2
screencapture -x "$SHOTS/hotkey-listening.png"
press_shortcut

text=""
for _ in $(seq 1 120); do
  text="$(osascript -e 'tell application "TextEdit" to get text of front document')"
  [[ -n "$text" ]] && break
  sleep 0.5
done
sleep 0.5
screencapture -x "$SHOTS/hotkey-pasted.png"
seconds=$(($(date +%s) - started))

python3 - "$OUT" "$text" "$seconds" <<'PY'
import json, sys
out, text, seconds = sys.argv[1], sys.argv[2], int(sys.argv[3])
json.dump({"pasted_text": text, "seconds": seconds}, open(out, "w"), indent=2)
print(json.dumps({"pasted_text": text, "seconds": seconds}, indent=2))
assert text.strip(), "nothing was pasted into TextEdit"
PY
echo "hotkey paste E2E passed"
