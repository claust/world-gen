#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
CAPTURE_DIR="$ROOT_DIR/captures"
mkdir -p "$CAPTURE_DIR"

STAMP="$(date +%Y%m%d-%H%M%S)"
OUT_PATH="${1:-$CAPTURE_DIR/world-gen-$STAMP.png}"
mkdir -p "$(dirname "$OUT_PATH")"

capture_with_peekaboo() {
  if ! command -v peekaboo >/dev/null 2>&1; then
    return 1
  fi

  # Prefer frontmost window so you can point this at a running world-gen instance quickly.
  peekaboo image --mode frontmost --path "$OUT_PATH" --format png >/dev/null
}

capture_with_screencapture() {
  if ! command -v screencapture >/dev/null 2>&1; then
    return 1
  fi

  # Fallback captures the current display. Put world-gen in front first.
  screencapture -x "$OUT_PATH"
}

if capture_with_peekaboo; then
  :
elif capture_with_screencapture; then
  :
else
  echo "Capture failed." >&2
  echo "Make sure Screen Recording permission is enabled for your terminal/automation host:" >&2
  echo "System Settings > Privacy & Security > Screen Recording" >&2
  echo "If using Peekaboo interactions, also enable Accessibility." >&2
  exit 1
fi

cp "$OUT_PATH" "$CAPTURE_DIR/latest.png"
echo "$OUT_PATH"
