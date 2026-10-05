#!/bin/sh
# Example uninstall hook. Data is preserved by default; PRAXIS_PURGE=1 asks the
# hook to remove it too.
set -eu
dir="$PRAXIS_DATA_DIR/$PRAXIS_PLUGIN_ID"
if [ "${PRAXIS_PURGE:-0}" = "1" ]; then
  rm -rf "$dir"
  printf 'hooks_demo data purged\n'
else
  rm -f "$dir/installed.txt"
  rmdir "$dir" 2>/dev/null || true
  printf 'hooks_demo removed (data preserved)\n'
fi
