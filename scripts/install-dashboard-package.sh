#!/bin/sh
# Build and install the Praxis dashboard package into PLUGINS_DIR/dashboard.
# Select it with DASHBOARD_PACKAGE=dashboard (Praxis then does not start the
# built-in dashboard; a build without the `dashboard` feature works too).
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PLUGINS_DIR=${1:-${PLUGINS_DIR:-$ROOT/plugins}}
PROFILE=${PROFILE:-release}
cd "$ROOT"
if [ "$PROFILE" = release ]; then
  cargo build --release -p praxis-dashboard
else
  cargo build -p praxis-dashboard
fi
TARGET="$PLUGINS_DIR/dashboard"
rm -rf "$TARGET.tmp"
mkdir -p "$TARGET.tmp/bin"
cp "target/$PROFILE/praxis-dashboard" "$TARGET.tmp/bin/praxis-dashboard"
cp -R static "$TARGET.tmp/static"
cp packages/dashboard/plugin.json "$TARGET.tmp/plugin.json"
rm -rf "$TARGET"
mv "$TARGET.tmp" "$TARGET"
echo "Installed dashboard package at $TARGET (DASHBOARD_PACKAGE=dashboard)"
