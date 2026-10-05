#!/bin/sh
# Build/install only the shell package, without linking it into Praxis.
set -eu
PRAXIS_SOURCE_ROOT=$(cd "$(dirname "$0")/.." && pwd)
PRAXIS_PACKAGE_PLUGINS=${1:-${PLUGINS_DIR:-$PRAXIS_SOURCE_ROOT/plugins}}
PROFILE=${PROFILE:-release}
case "$PROFILE" in release|debug) ;; *) echo "PROFILE must be release or debug" >&2; exit 1 ;; esac
cd "$PRAXIS_SOURCE_ROOT"
if [ "$PROFILE" = release ]; then
  cargo build --locked --release -p praxis-shell
else
  cargo build --locked -p praxis-shell
fi
PRAXIS_PACKAGE_TARGET="$PRAXIS_PACKAGE_PLUGINS/shell"
if [ -e "$PRAXIS_PACKAGE_TARGET" ] || [ -L "$PRAXIS_PACKAGE_TARGET" ]; then
  echo "Package already exists: $PRAXIS_PACKAGE_TARGET. Preserve customized files and upgrade its binary explicitly while Praxis is stopped." >&2
  exit 1
fi
mkdir -p "$PRAXIS_PACKAGE_PLUGINS"
PRAXIS_PACKAGE_STAGE=$(mktemp -d "$PRAXIS_PACKAGE_PLUGINS/.shell.XXXXXX")
trap 'rm -rf "$PRAXIS_PACKAGE_STAGE"' EXIT HUP INT TERM
mkdir -p "$PRAXIS_PACKAGE_STAGE/bin"
install -m 755 "${CARGO_TARGET_DIR:-$PRAXIS_SOURCE_ROOT/target}/$PROFILE/praxis-shell" "$PRAXIS_PACKAGE_STAGE/bin/praxis-shell"
cp packages/shell/plugin.json "$PRAXIS_PACKAGE_STAGE/plugin.json"
mv -T "$PRAXIS_PACKAGE_STAGE" "$PRAXIS_PACKAGE_TARGET"
trap - EXIT HUP INT TERM
echo "Installed shell at $PRAXIS_PACKAGE_TARGET; restart Praxis to load it."
echo "Foreground execute_terminal needs no further setup. To enable durable background"
echo "jobs, set SHELL_SERVICE_EXECUTABLE=plugins/shell/bin/praxis-shell (relative to"
echo "ROOT_DIR or absolute) before restarting."
