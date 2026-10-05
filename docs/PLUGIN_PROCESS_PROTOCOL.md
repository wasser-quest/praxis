# Installed feature workers: process protocol v1

The VM plugin can run as `praxis-vm-service`, separately from the Praxis process.
A Praxis binary built with `--no-default-features` can use it without linking the
QEMU implementation. The default compatibility build keeps its native backend
unless the operator sets `VM_SERVICE_EXECUTABLE`. See [installation commands](INSTALLATION_PRESETS.md#install-the-headless-vm-worker).

Roadmap PR 3 now includes the headless transport and the package-owned web
contribution: administration, VNC, embedded noVNC and a dashboard page. The
independent VM CLI and screenshot delivery also work through the installed package;
complete guest ownership/upgrade policy remains. Manifest v2 and immutable
package publication remain later work. A package may now declare its installed
worker directly on a `service` handler (`executable` plus optional `args`); the
host resolves the path inside the package at load time and binds it at startup,
so no bespoke environment variable is required. Explicit bindings (VM, shell)
still win when already registered, and models cannot select an executable. The
shell worker owns `run_background`/`background_status`
job state, while foreground `execute_terminal` stays on the one-shot executable
transport. See
[shell package installation](TOOL_PACKAGES.md#independently-installed-shell-and-background-jobs).

[shell package installation](TOOL_PACKAGES.md#independently-installed-shell-and-background-jobs).

## Declaring an installed worker

A host-agnostic package can bind its own worker from the manifest:

```json
{
  "name": "probe",
  "tools": [{
    "name": "probe_echo",
    "handler": {
      "type": "service", "service": "probe", "operation": "probe_echo",
      "api_version": 1, "timeout_secs": 30,
      "executable": "bin/probe-service", "args": ["--stdio"]
    },
    "parameters": {"type": "object", "properties": {}}
  }]
}
```

Every tool of one `(owner, service)` must agree on the executable and args, or
activation fails before any effect. The loader canonicalizes the path inside the
package (no escape), and registration is effect-free; `praxis run` initializes
the binding after registration and before inference. The worker receives
`data_dir`, `root_dir` and `workspace` in its `hello` initialization and the
authenticated caller in each `invoke`. A missing executable, mismatched
handshake, crash or cancellation fails closed without host fallback or replay.

## Boundary

`crates/praxis-plugin-api` supplies a host client and worker server, without QEMU,
database, dashboard or inference dependencies. Each worker gets dedicated
stdin/stdout pipes. Frames contain a four-byte big-endian length followed by JSON,
bounded to 2 MiB including the envelope. Core capability input/results retain
their 1 MiB limits. Unknown fields, malformed JSON, oversized lengths, truncated
frames and mismatched response identities fail closed.

The startup handshake checks protocol version 1, owner, service ID, a fresh nonce
and the exact operation/control tables. Native invocation API version 1 remains a
separate interface: the manifest's `api_version`. Tool calls use only the pipes.
The web contribution binds a private authenticated loopback listener at an
ephemeral port; it is never exposed directly as the public dashboard. See
[web contributions](PLUGIN_WEB_CONTRIBUTIONS.md).

| Request | Purpose |
| --- | --- |
| `hello` | Host-selected initialization and declaration handshake |
| `invoke` | Host caller envelope plus separate model arguments |
| `control` | Host lifecycle/metadata/delivery operation; VM exposes `autostart`, `web_info` and `capture` |
| `health` | Worker responsiveness without starting a guest |
| `cancel` | Interrupt the matching in-flight request |
| `shutdown` | Stop this worker instance |

Request IDs increase monotonically within the nonce. Responses must match both;
stale results cannot be accepted by a replacement binding. There is one in-flight
request per worker and a bounded queue. Host deadlines include queue time.
Handshake/health/shutdown have five-second budgets; invocations use the host-issued
budget, at most 300 seconds.

## Identity, policy and evidence

Praxis constructs the caller envelope from `InvocationContext`: authenticated
user/session, task/call ID, owner, pinned registry revision, workspace, state and
remaining deadline. Model arguments cannot issue that authority. Preferences come
from that user's context. Credentials resolve only from the declared grant for
the guest selected by the handler: install uses `vm_name`, other tools use `name`.
Neither the whole secret store nor provider/admin environment reaches the worker.
The VM launch environment passes `PATH` for QEMU utilities and necessary Windows
process directories on Windows.

Registration validates the executable without spawning it. Host startup
initializes/handshakes it before inference; unready/dead bindings are hidden from
discovery and fail preflight. Startup failure is a setup error. Configured guest
autostart still reports a non-fatal failure if QEMU/guest setup is unavailable.

`ROOT_DIR` resolves a relative `VM_SERVICE_EXECUTABLE`; the worker's cwd is that
installation. `DATA_DIR` retains host-cwd semantics and is sent as an absolute
path. `WORKSPACE_DIR` remains the separately pinned verified host project.

`capture` accepts only a guest name and typed, host-resolved preferences, returning
an image artifact path or null. The host bounds capture to ten seconds, validates
the returned storage path and keeps delivery in the service drain/cancellation
scope. This host-only control grants no model workspace authority or credentials.
It is an explicit screenshot request; `screenshot_enabled` still controls optional
automatic capture after tool effects.

Operator CLI commands use a separate `--cli` entry point, ordinary stdout/stderr
and an exit status. The host forwards literal OS arguments, public VM settings on
bounded stdin, and the same environment allow-list. The VM package owns parsing,
help and effects; no host database/UI/inference service is started. Models cannot
choose this entry point or executable. See [commands and attachment limits](VM_PLUGIN.md#operator-cli-and-screenshots).

The worker returns data, including guest-scoped results with `verified: false`.
Only the host contract verifier/ledger can issue receipts that satisfy guards.
A guest command cannot prove that a host Cargo project changed, built or tested.
The process boundary is not an OS sandbox: installed executables remain
operator-trusted, running under the host OS user.

## Cancellation, disable and recovery

Dropping/timing out an admitted call sends a bounded cooperative cancel. The
worker drops its action future before acknowledging, then ends the instance.
Unfinished VM startup consequently kills only its newly spawned QEMU child.
A nonresponsive worker is terminated after a 500 ms acknowledgement budget.
Crashes, invalid replies and cancellation end availability; ordinary operation
errors use controlled messages and keep a responsive worker alive.

Disabling blocks new calls through every registry snapshot, drains active calls,
then signals cancellation when grace expires. Exhausting the cleanup budget still
signals the owned worker. Shutdown reaps that worker; it does not kill committed
guests, delete disks/shared files or claim rollback of network effects. A
forced/incomplete shutdown remains a failed cleanup result.

There is no automatic restart or action replay. After a crash/cancellation, the
operator restarts Praxis to create a fresh binding and task revision. Existing
guests can be reattached using the native manager's QMP validation. Partial
external effects require inspection; a missing reply proves neither failure nor
rollback. Worker stdout contains only protocol frames. Raw implementation errors
and stderr are not returned to the model or written to host logs.

## Verification

```bash
cargo test -p praxis-plugin-api --locked
cargo test -p praxis-vm -p praxis-vm-web -p praxis-vm-worker --locked
cargo test --lib --locked --no-default-features runtime::vm_process_tests
cargo test --lib --locked runtime::feature_tests
cargo tree -p praxis --no-default-features
```

Tests use local process/QMP fixtures and a controlled startup child, without real
QEMU guests or live inference. They cover identity, credential selection,
environment isolation, handshake/version failures, bounds, idle/in-flight crash
detection, stale identities, deadlines including queue time, cancellation cleanup,
live disable, package web authentication/VNC disconnect and rejection of guest
completion evidence. Real guest installation and interactive VNC keyboard/mouse
remain operator smoke checks.
