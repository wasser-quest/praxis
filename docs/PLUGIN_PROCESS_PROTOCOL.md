# Installed feature workers: process protocol v1

The VM plugin can run as `praxis-vm-service`, separately from the Praxis process.
A Praxis binary built with `--no-default-features` can use it without linking the
QEMU implementation. The default compatibility build keeps its native backend
unless the operator sets `VM_SERVICE_EXECUTABLE`. See [installation commands](INSTALLATION_PRESETS.md#install-the-headless-vm-worker).

This is the headless transport part of roadmap PR 3. VM routes, VNC, the dashboard
page and the independent VM CLI are not transported yet. Manifest v2, immutable
package publication and generic process declarations remain later work. Only the
VM adapter selects a worker today; models cannot select an executable.

## Boundary

`crates/praxis-plugin-api` supplies a host client and worker server, without QEMU,
database, dashboard or inference dependencies. Each worker gets dedicated
stdin/stdout pipes. Frames contain a four-byte big-endian length followed by JSON,
bounded to 2 MiB including the envelope. Core capability input/results retain
their 1 MiB limits. Unknown fields, malformed JSON, oversized lengths, truncated
frames and mismatched response identities fail closed.

The startup handshake checks protocol version 1, owner, service ID, a fresh nonce
and the exact operation/control tables. Native invocation API version 1 remains a
separate interface: the manifest's `api_version`. No network listener or
unauthenticated socket is created.

| Request | Purpose |
| --- | --- |
| `hello` | Host-selected initialization and declaration handshake |
| `invoke` | Host caller envelope plus separate model arguments |
| `control` | Operator lifecycle operation; VM exposes configured autostart |
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
cargo test -p praxis-vm --locked
cargo test --lib --locked --no-default-features runtime::vm_process_tests
cargo test --lib --locked runtime::feature_tests
cargo tree -p praxis --no-default-features
```

Tests use local process/QMP fixtures and a controlled startup child, without real
QEMU guests or live inference. They cover identity, credential selection,
environment isolation, handshake/version failures, bounds, idle/in-flight crash
detection, stale identities, deadlines including queue time, cancellation cleanup,
live disable and rejection of guest completion evidence. Interactive guest/VNC
validation belongs to the later UI slice.
