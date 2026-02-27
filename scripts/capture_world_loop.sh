#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SNAPSHOT_SCRIPT="$ROOT_DIR/scripts/capture_world.sh"

INTERVAL_SECONDS="${1:-2}"
COUNT="${2:-10}"

if [ ! -x "$SNAPSHOT_SCRIPT" ]; then
  chmod +x "$SNAPSHOT_SCRIPT"
fi

for ((i = 1; i <= COUNT; i++)); do
  "$SNAPSHOT_SCRIPT"
  if [ "$i" -lt "$COUNT" ]; then
    sleep "$INTERVAL_SECONDS"
  fi
done
