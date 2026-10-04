# VM package

`crates/praxis-vm` owns QEMU/QMP/serial operations, the 25 VM tool implementations,
screen capture and credential-directory preparation. It has no Praxis database,
provider, POML or dashboard dependency and does not read `DATA_DIR`, socket mode or
user preferences from global process state. The host supplies configuration,
authenticated invocation identity, user preferences and explicit credentials.

`plugins/vm/plugin.json` declares the tools through native invocation API v1.
`src/runtime/vm.rs` binds the implementation, selects the legacy execution backend
and supplies the user's keyboard/screenshot settings. `crates/praxis-vm/src/cli.rs`
owns CLI parsing and execution; `src/runtime/vm/cli.rs` forwards operator arguments
to the selected package, including in builds without the engine.
Creating/registering the backend does not create guest directories or processes.
The optional `praxis-vm-service` worker initializes separately at host startup.
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
VM plus `VM_ENABLED=true` requires an installed worker selected by
`VM_SERVICE_EXECUTABLE`; otherwise it fails setup before VM initialization.

For headless process hosting, build/install `praxis-vm-service` and set
`VM_SERVICE_EXECUTABLE` to an absolute path or a path relative to `ROOT_DIR`.
The same VM manifest/tools work with a core without the QEMU crate. Leaving
this setting unset preserves the native backend. Follow
[the installation commands](INSTALLATION_PRESETS.md#install-the-headless-vm-worker).

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

## Operator CLI and screenshots

With the worker installation and `.env` from [installation presets](INSTALLATION_PRESETS.md#install-the-headless-vm-worker):

```bash
cd "$PRAXIS_INSTALL"
./praxis vm --help
./praxis vm status
./praxis vm list-isos
./praxis vm disk list
./praxis vm add-iso /absolute/path/to/linux.iso --name linux
./praxis vm start --name praxis-vm --iso linux
./praxis vm screenshot --name praxis-vm --output /absolute/path/to/screenshot.png
./praxis vm shell --name praxis-vm --timeout 30 "uname -a"
./praxis vm shutdown --name praxis-vm
# Immediate QMP quit request, if needed:
./praxis vm stop --name praxis-vm --force
```

Use the same `DATA_DIR`, architecture and socket mode as the running service.
The CLI does not start the dashboard, load the database, unlock credentials or
initialize inference. It uses `praxis-vm-service --cli` with literal arguments and
bounded public settings on stdin; no shell wrapper or provider/admin environment
is forwarded. Without `VM_SERVICE_EXECUTABLE`, a compatibility build uses the same
package implementation in-process. A configured worker failure never selects the
native engine. Commands report failures through a nonzero exit status.

Existing guests attach through their configured QMP endpoint after its name is
checked. Missing guests are not started by status/screenshot/stop commands. Unix
status discovers per-guest sockets in `DATA_DIR/vm`; TCP status currently checks
the default `praxis-vm` endpoint. Per-guest TCP metadata/port allocation belongs to
the remaining ownership/recovery work. Shutdown/force-stop acknowledge the QMP
request; they do not claim independently verified guest termination. Manual
starts receive no credential grants, while read-only attachment leaves existing
guest credentials untouched. CLI screenshots save PNG bytes; CD commands execute
the actual QMP media operation.

Automatic screenshots work with either backend. Enable them for the user:

```json
{"settings":{"vm_screenshot_enabled":true,"vm_screenshot_limit":100}}
```

Explicit screenshot delivery can capture even when automatic capture is disabled.
The worker receives that user's host-resolved preferences, never an operand-chosen
identity or data directory. Capture/delivery shares the binding's drain and forced
cancellation, has a ten-second capture budget, and cannot restart a dead worker.
Returned files must stay in the selected guest's screenshot directory; traversal
and escaped symlinks are rejected. Image context loading also checks the live
binding and storage path, including with a core-only host. Retention runs after
each saved image, counts PNG and PPM files, and keeps at least the current image
when the configured limit is zero. Guest results remain `verified: false`.

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

The engine supports an optional **native Rust package** and an independently
installed **headless worker**. The [process protocol](PLUGIN_PROCESS_PROTOCOL.md)
handles health, caller scope, deadlines, cancellation and worker crashes. Stopped
instances are not restarted/replayed automatically; restart Praxis for a fresh
binding. Installing a manifest alone still does not install an executable.

The `praxis-vm-web` package owns VM administration, VNC, the dashboard page and
noVNC assets/licenses. `praxis-vm-worker` hosts it alongside the engine; the
native compatibility backend uses the same web implementation. Host authentication
and a generic bounded proxy keep public authority outside the worker. See
[web contributions](PLUGIN_WEB_CONTRIBUTIONS.md). The VM page works with either
backend, including a core built without the VM crates. Manual dashboard starts
use explicit request layout (default `us`) and no implicit credential grants.

Per-user VM ownership and complete upgrade/recovery policy are remaining PR 3
work. Update host and worker together and refresh dashboard assets; older workers
lack the required `capture` control or the independent `--cli` entry point.

## Verification

```bash
cargo test -p praxis-vm -p praxis-vm-web -p praxis-vm-worker --locked
cargo test -p praxis-plugin-api --locked
cargo test --lib --locked --no-default-features runtime::vm_process_tests
cargo test --lib --locked runtime::vm_tests
cargo test --lib --locked --no-default-features dashboard::extensions
node tests/test_dashboard_extensions.js
cargo test --lib --locked --no-default-features runtime::vm_tests
cargo tree -p praxis --no-default-features
```

Tests use a local QMP fixture and a controlled child-process stub, including
reattachment and cancelled-startup cleanup. They do not boot a real guest or use
live model inference. A real QEMU guest, OS installation and interactive noVNC
keyboard/mouse remain operator smoke checks.

## Guest ownership and recovery

Every guest has a persisted record at `DATA_DIR/vm/<name>/guest.json`, written
atomically (temp file, fsync, rename). It stores the owner, explicit shares,
the full guest configuration (disk path, QMP/serial sockets or ports, VNC port,
shared folders), the last known QEMU pid, any in-progress lifecycle operation
and a bounded history of interrupted operations. Records sit beside the disk,
so disabling, upgrading or replacing the VM package never discards them.

### Access policy

| Principal | Use (input, shell, screenshots, VNC, files, clipboard, snapshot create/list) | Manage (stop, reboot, CD, shared folders, reinstall, snapshot restore/delete, sharing) |
|---|---|---|
| Owner | yes | yes |
| Explicitly shared user | yes | no |
| `*` share (all authenticated users) | yes | no |
| Operator (dashboard token, autostart) | yes | yes, plus ownership transfer |
| Anyone else | denied | denied |

- Tools: the authenticated task user is the principal. Calls on another
  user's guest return `outcome: "denied"` before any QMP/serial effect. Only
  `vm_start`/`vm_install` may claim an unused name, and only after input
  validation; other operations on unknown guests are denied without creating
  storage.
- HTTP routes and VNC: the host proxy sets `x-praxis-principal`
  (`operator` or `user:<id>`) on freshly built upstream requests; a browser
  cannot forward its own. The package rejects requests without it (fail
  closed). VNC is authorized before any TCP connection to QEMU.
- Screenshots: the `capture` control requires the host-resolved `user` and
  checks Use access; listing (`GET /api/plugins/vm`, `/api/plugins/vm/guests`)
  is filtered, and sharers do not see owner or share lists.
- Sharing: `POST /api/plugins/vm/share {name,user,grant}` (owner/operator) and
  `POST /api/plugins/vm/owner {name,owner}` (operator only). The worker exposes
  the same as `guests`/`share`/`transfer` controls.

### Migration of existing guests

On first start after upgrade, recovery adopts each guest directory that has a
`disk.qcow2` but no record: owner `admin`, shared with `*`. This keeps the
historical single-tenant behaviour (every user could use the default
`praxis-vm`) while making it visible. Operators narrow it by removing the `*`
share or transferring ownership. A user cannot claim a legacy guest by starting
it. A newly autostarted default guest is created the same way. Unreadable or
unsupported records are left untouched and the guest is skipped.

### Restart, disable and upgrade recovery

When the worker (or native binding) initializes it runs recovery before serving
tools or routes:

1. Adopt legacy guests as above.
2. Convert any `pending` lifecycle operation into an `interrupted` entry
   (`"interrupted; guest effects not compensated"`). Nothing is replayed.
3. Reattach to still-running QEMU through the persisted endpoints; the QMP
   `query-name` must match the guest name.

Recovery never spawns QEMU, kills a process or deletes a disk. Later starts
reuse the recorded disk and endpoints rather than re-deriving ports. Disabling
or upgrading the package drains sessions but leaves guests running and disks in
place. There is no implicit deletion path; removing a guest's data remains an
explicit operator action on `DATA_DIR/vm/<name>`.

A failed first `vm_start` still leaves the claimed record so the owner can
retry; the operator can transfer or remove it.
