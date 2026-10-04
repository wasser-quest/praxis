# Native VM package

`crates/praxis-vm` owns QEMU/QMP/serial operations, the 25 VM tool implementations,
screen capture and credential-directory preparation. It has no Praxis database,
provider, POML or dashboard dependency and does not read `DATA_DIR`, socket mode or
user preferences from global process state. The host supplies configuration,
authenticated invocation identity, user preferences and explicit credentials.

`plugins/vm/plugin.json` declares the tools through native invocation API v1.
`src/runtime/vm.rs` binds the implementation, selects the legacy execution backend
and supplies the user's keyboard/screenshot settings. CLI commands are contributed
by `src/runtime/vm/cli.rs`; the entry point registers them only in builds with VM.
Creating/registering the backend does not create directories or start processes.
Guest names use 1–80 ASCII letters, digits, underscores, hyphens or dots; storage
directory names and `.`/`..` are reserved.

## Installation and compatibility

Use `--features compatibility` (the normal default) or `--features vm`, install
the public assets, configure `VM_ENABLED=true` and install QEMU. The compatibility
preset can explicitly enable the 25 VM tool flags. See the exact commands in
[installation presets](INSTALLATION_PRESETS.md).

Historical flags in `DATA_DIR/tools.json` remain effective until an explicit
plugin flag overrides them. The tools have one owner, `vm`; they are no longer
also executed as builtins. Disabled flags remain disabled on subsequent starts.
The host's selected backend is fixed in the registry snapshot. An unavailable or
disabled guest binding cannot fall back to running an agent command on the host.

Configured autostart still starts `praxis-vm`. A reachable existing guest is
reattached via QMP; the host no longer kills arbitrary processes by matching a VM
name. Cancelled, unfinished startup kills only its newly spawned child. Committed
guests and their disks survive native-service drain/host shutdown. A build without
VM plus `VM_ENABLED=true` fails with a setup error before VM initialization.

## Execution scope

VM operations return guest-scoped results, including the authenticated user/task,
the operation, output and `verified: false`. They cannot create a verified host
workspace receipt or satisfy host coding completion guards. Shell/network/package
effects may persist after cancellation; VM snapshots do not undo external network
effects. Native invocation deadlines cap a call at the declared 300-second budget.

In `VM_MODE=vm`, the agent's legacy terminal/read/write/edit aliases use the
registered guest backend. Chat retains host aliases. Contracted Rust capabilities
remain pinned to the host workspace. Workflows and tool flags control access;
shared folders intentionally expose their configured host resources.

Capture/retention is an optional VM-service hook. The agent consumes actual
returned screenshot artifacts. `settings.vm_keyboard_layout`,
`settings.vm_screenshot_enabled` and `settings.vm_screenshot_limit` are resolved
for the authenticated caller, with no `default`-user database fallback.

## Credential grants

Edit only these fields in the installed VM manifest, retaining its tool table:

```json
{
  "secrets": ["vm_git_token"],
  "context": {
    "credential_grants": {
      "praxis-vm": { "VM_GIT_TOKEN": "vm_git_token" }
    }
  }
}
```

Set `vm_git_token` through Dashboard → Secrets, then restart. Each mapping selects
a declared plugin secret and a credential filename for one guest. The default
mapping is empty. No automatic provider/gateway/admin export or environment loader
remains. Linux guests can mount the read-only `praxis-secrets` 9p share at
`/run/secrets` and read the required file. Mounting is a guest setup action.

Credential preparation removes obsolete files from that guest's credential
directory, including old blanket-injection files, but preserves disks/shared
files. Unix directories/files use `0700`/`0600`; symlinked credential paths and
invalid names fail before credential writes. Manual VM CLI starts have no implicit
credential grants; configured runtime autostart and native tools use the manifest.

## Transitional boundary and next step

This is an optional **native Rust package**. Installing its manifest alone cannot
add QEMU code to a binary built without VM. Independent executable/IPC installation,
VM ownership policy, lifecycle recovery and the VM routes/noVNC UI package are the
next plan step. Current dashboard/delivery adapters obtain the explicitly bound
instance through a host-only bridge; they cannot lazy-initialize a global manager.
The legacy `/websockify` alias now uses the authenticated VNC adapter, and clients
send the dashboard token. Refresh updated dashboard assets after upgrade.

## Verification

```bash
cargo test -p praxis-vm --locked
cargo test --lib --locked runtime::vm_tests
cargo test --lib --locked dashboard::routes::vm_tests
cargo test --lib --locked --no-default-features runtime::vm_tests
cargo tree -p praxis --no-default-features
```

Tests use a local QMP fixture and a controlled child-process stub, including
reattachment and cancelled-startup cleanup. They do not boot a real guest or use
live model inference. A real QEMU guest, OS installation and interactive noVNC
keyboard/mouse remain operator smoke checks.
