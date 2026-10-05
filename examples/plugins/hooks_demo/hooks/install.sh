#!/bin/sh
# Example install hook. Runs from the published plugin directory with a clean
# environment plus PRAXIS_* variables. Must be idempotent and safe to re-run.
set -eu
marker="$PRAXIS_DATA_DIR/$PRAXIS_PLUGIN_ID"
mkdir -p "$marker"
printf 'installed %s\n' "$PRAXIS_PLUGIN_VERSION" > "$marker/installed.txt"
printf 'hooks_demo ready: data at %s\n' "$marker"
