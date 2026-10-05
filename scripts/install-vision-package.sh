#!/bin/sh
# Build/install only the vision package, without linking it into Praxis.
set -eu
PRAXIS_SOURCE_ROOT=$(cd "$(dirname "$0")/.." && pwd)
PRAXIS_PACKAGE_PLUGINS=${1:-${PLUGINS_DIR:-$PRAXIS_SOURCE_ROOT/plugins}}
PROFILE=${PROFILE:-release}
case "$PROFILE" in release|debug) ;; *) echo "PROFILE must be release or debug" >&2; exit 1 ;; esac
cd "$PRAXIS_SOURCE_ROOT"
if [ "$PROFILE" = release ]; then
  cargo build --locked --release -p praxis-vision
else
  cargo build --locked -p praxis-vision
fi
PRAXIS_PACKAGE_TARGET="$PRAXIS_PACKAGE_PLUGINS/vision"
if [ -e "$PRAXIS_PACKAGE_TARGET" ] || [ -L "$PRAXIS_PACKAGE_TARGET" ]; then
  echo "Package already exists: $PRAXIS_PACKAGE_TARGET. Preserve customized files and upgrade its binary explicitly while Praxis is stopped." >&2
  exit 1
fi
mkdir -p "$PRAXIS_PACKAGE_PLUGINS"
PRAXIS_PACKAGE_STAGE=$(mktemp -d "$PRAXIS_PACKAGE_PLUGINS/.vision.XXXXXX")
trap 'rm -rf "$PRAXIS_PACKAGE_STAGE"' EXIT HUP INT TERM
mkdir -p "$PRAXIS_PACKAGE_STAGE/bin"
install -m 755 "${CARGO_TARGET_DIR:-$PRAXIS_SOURCE_ROOT/target}/$PROFILE/praxis-vision" "$PRAXIS_PACKAGE_STAGE/bin/praxis-vision"
cp packages/vision/plugin.json "$PRAXIS_PACKAGE_STAGE/plugin.json"
mv -T "$PRAXIS_PACKAGE_STAGE" "$PRAXIS_PACKAGE_TARGET"
trap - EXIT HUP INT TERM
echo "Installed vision at $PRAXIS_PACKAGE_TARGET; restart Praxis to load it."
