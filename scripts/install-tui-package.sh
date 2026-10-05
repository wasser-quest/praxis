#!/bin/sh
# Build/install only the TUI frontend package (praxis-tui).
set -eu
PRAXIS_SOURCE_ROOT=$(cd "$(dirname "$0")/.." && pwd)
PRAXIS_PACKAGE_PLUGINS=${1:-${PLUGINS_DIR:-$PRAXIS_SOURCE_ROOT/plugins}}
PROFILE=${PROFILE:-release}
case "$PROFILE" in release|debug) ;; *) echo "PROFILE must be release or debug" >&2; exit 1 ;; esac
cd "$PRAXIS_SOURCE_ROOT"
if [ "$PROFILE" = release ]; then
  cargo build --locked --release -p praxis --bin praxis-tui
else
  cargo build --locked -p praxis --bin praxis-tui
fi
PRAXIS_PACKAGE_TARGET="$PRAXIS_PACKAGE_PLUGINS/tui"
if [ -e "$PRAXIS_PACKAGE_TARGET" ] || [ -L "$PRAXIS_PACKAGE_TARGET" ]; then
  echo "Package already exists: $PRAXIS_PACKAGE_TARGET. Preserve customized files and upgrade its binary explicitly while Praxis is stopped." >&2
  exit 1
fi
mkdir -p "$PRAXIS_PACKAGE_PLUGINS"
PRAXIS_PACKAGE_STAGE=$(mktemp -d "$PRAXIS_PACKAGE_PLUGINS/.tui.XXXXXX")
trap 'rm -rf "$PRAXIS_PACKAGE_STAGE"' EXIT HUP INT TERM
mkdir -p "$PRAXIS_PACKAGE_STAGE/bin"
install -m 755 "${CARGO_TARGET_DIR:-$PRAXIS_SOURCE_ROOT/target}/$PROFILE/praxis-tui" "$PRAXIS_PACKAGE_STAGE/bin/praxis-tui"
cp packages/tui/plugin.json "$PRAXIS_PACKAGE_STAGE/plugin.json"
mv -T "$PRAXIS_PACKAGE_STAGE" "$PRAXIS_PACKAGE_TARGET"
trap - EXIT HUP INT TERM
echo "Installed tui at $PRAXIS_PACKAGE_TARGET; run $PRAXIS_PACKAGE_TARGET/bin/praxis-tui or add it to PATH."
