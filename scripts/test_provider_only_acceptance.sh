#!/bin/sh
# Handoff §6D acceptance: provider-only chat, verified Rust and language
# teaching must work on a build with no VM, UI or media.
#
# The three flows run on the core-only host (--no-default-features): no QEMU
# or VM tools, no dashboard or static UI, no media pipeline and no channel
# worker is even compiled in.
set -eu
cd "$(dirname "$0")/.."
export CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"

echo "== core-only host: provider-only chat"
cargo test --locked --no-default-features --lib -- \
    message_tool_loop_tests task_tests skill_tool_loop_tests

echo "== core-only host: verified Rust (contracts, checks and rollback)"
cargo test --locked --no-default-features --lib -- \
    capability_dispatch_tests contract_tests

echo "== core-only host: language teaching (reply guard and profile persistence)"
cargo test --locked --no-default-features --lib -- \
    learning_flow_tests memory_profile

echo "PASS: provider-only chat, verified Rust and language teaching work without VM/UI/media"
