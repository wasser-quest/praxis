#!/bin/sh
# Build the legacy file tools package and install it through the Praxis lifecycle.
#
# The build stages everything inside packages/legacy_file_ops/, so plugins and package
# builds are one flow: after this runs (or after a plain 'cargo build -p praxis-legacy-file-ops'
# plus placing the binary), 'praxis plugin install ./packages/legacy_file_ops' works
# directly. This script does both steps: it runs the installer, then the same
# 'praxis plugin install' you could run yourself.
set -eu
ROOT=$(cd "$(dirname "$0")/.." && pwd)
PLUGINS_DIR=${1:-${PLUGINS_DIR:-$ROOT/plugins}}
export PLUGINS_DIR
PROFILE=${PROFILE:-release}
case "$PROFILE" in release|debug) ;; *) echo "PROFILE must be release or debug" >&2; exit 1 ;; esac
cd "$ROOT"
if [ "$PROFILE" = release ]; then
  cargo build --locked --release -p praxis-legacy-file-ops
else
  cargo build --locked -p praxis-legacy-file-ops
fi

# Stage the package where its manifest expects the binary.
STAGE=packages/legacy_file_ops
mkdir -p "$STAGE/bin"
install -m 755 "${CARGO_TARGET_DIR:-$ROOT/target}/$PROFILE/praxis-legacy-file-ops" "$STAGE/bin/praxis-legacy-file-ops"

# Install through the kernel: manifest validation, PLUGIN_HOOKS policy and
# praxis.lock.json records — exactly what a manual 'praxis plugin install' does.
PRAXIS=${PRAXIS:-}
if [ -z "$PRAXIS" ]; then
  for candidate in "${CARGO_TARGET_DIR:-$ROOT/target}/$PROFILE/praxis" "$(command -v praxis 2>/dev/null || true)"; do
    if [ -n "$candidate" ] && [ -x "$candidate" ]; then PRAXIS=$candidate; break; fi
  done
fi
if [ -n "$PRAXIS" ]; then
  exec "$PRAXIS" plugin install "$STAGE"
fi
echo "Built $STAGE. Install it with: praxis plugin install $STAGE"
