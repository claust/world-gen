#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
CAPTURE_DIR="$ROOT_DIR/captures"
mkdir -p "$CAPTURE_DIR"

STAMP="$(date +%Y%m%d-%H%M%S)"
OUT_PATH="${1:-$CAPTURE_DIR/world-gen-$STAMP.png}"
mkdir -p "$(dirname "$OUT_PATH")"

print_macos_permission_help() {
  cat >&2 <<'EOF'
Check macOS permissions:
1. Run: peekaboo list permissions
2. In System Settings > Privacy & Security:
   - enable Screen Recording
   - enable Accessibility
Restart your terminal app after changing permissions.
EOF
}

capture_with_peekaboo() {
  if ! command -v peekaboo >/dev/null 2>&1; then
    echo "Peekaboo is not installed. Falling back to full-display capture." >&2
    return 1
  fi

  local peekaboo_stderr
  peekaboo_stderr="$(mktemp)"

  # Prefer frontmost window so you can point this at a running world-gen instance quickly.
  if peekaboo image --mode frontmost --path "$OUT_PATH" --format png >/dev/null 2>"$peekaboo_stderr"; then
    rm -f "$peekaboo_stderr"
    return 0
  fi

  local details
  details="$(cat "$peekaboo_stderr")"
  rm -f "$peekaboo_stderr"

  echo "Peekaboo could not capture the frontmost window." >&2
  if [ -n "$details" ]; then
    echo "Peekaboo output: $details" >&2
  fi

  if printf "%s" "$details" | grep -Eiq "permission|accessibility|screen recording|not authorized|denied"; then
    print_macos_permission_help
  fi

  return 1
}

capture_with_screencapture() {
  if ! command -v screencapture >/dev/null 2>&1; then
    echo "The 'screencapture' command is not available on this system." >&2
    return 1
  fi

  local screencapture_stderr
  screencapture_stderr="$(mktemp)"

  # Fallback captures the current display. Put world-gen in front first.
  if screencapture -x "$OUT_PATH" 2>"$screencapture_stderr"; then
    rm -f "$screencapture_stderr"
    return 0
  fi

  local details
  details="$(cat "$screencapture_stderr")"
  rm -f "$screencapture_stderr"

  echo "Fallback display capture failed." >&2
  if [ -n "$details" ]; then
    echo "screencapture output: $details" >&2
  fi

  if printf "%s" "$details" | grep -Eiq "permission|screen recording|not authorized|denied"; then
    echo "Screen Recording permission is likely missing for your terminal app." >&2
    echo "Open System Settings > Privacy & Security > Screen Recording and allow it." >&2
  fi

  return 1
}

if capture_with_peekaboo; then
  :
elif capture_with_screencapture; then
  echo "Captured the full display as a fallback. Install/authorize Peekaboo for frontmost-window capture." >&2
  :
else
  echo "Capture failed." >&2
  print_macos_permission_help
  exit 1
fi

cp "$OUT_PATH" "$CAPTURE_DIR/latest.png"
echo "$OUT_PATH"
