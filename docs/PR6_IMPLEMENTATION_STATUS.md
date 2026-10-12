# PR6 implementation status — incomplete

This is an implementation checkpoint, not a completion claim. See
[the roadmap](../plan/PLUGINIZATION.md) and
[the pluginization handoff](PLUGINIZATION_HANDOFF.md).

## Implemented foundation

- `provides.providers`, `provides.channels`, and `provides.media` reserve
  independent non-tool ownership namespaces. A package does not need to expose
  a dummy model-facing tool to declare these contributions.
- Process declarations contain package-relative executable, bounded arguments,
  versioned protocol, explicit capabilities, secret references, and timeout.
  Unknown fields, capabilities, duplicate IDs, invalid versions, escaping paths,
  non-executable workers (Unix), and undeclared secret references are rejected.
- Provider/media declarations request the runtime trust role; channel declarations
  request the channel role. Manifest role validation does not grant operator
  trust: the existing trust store remains authoritative.
- Registry revision rehashes resolved contribution implementation bytes (maximum
  128 MiB per worker); same-size/preserved-mtime changes cannot reuse a cached
  digest. Checked registration revalidates role, secret grants and resolution.
  Declaration loading does not launch a worker or pass any credentials.
- `praxis-provider-api::wire` defines checked versioned streaming envelopes,
  explicit thinking preservation, request/provider/generation-bound opaque
  continuation, safe error fields, terminal attempt, sequence/terminal checks,
  and bounded frame/event/aggregate accounting. Legacy Rust traits are unchanged.

These declarations are **not yet runtime bindings**. Bounded length-prefixed transport, exact readiness negotiation, and private-stdio
process ownership are implemented as foundations; these are **not yet a complete
provider bridge**. Bare serde decoding is not a substitute for the checked codecs.
Existing compiled provider/channel/media paths remain in use; this checkpoint
has neither switched nor installed a release binary.

## Bounded provider preparation checkpoint

Per-invocation registry/secrets snapshots now use cancellable, deadline-bounded
lock acquisition. Filesystem authorization/hashing runs off-executor, with one
in-flight blocking preparation per adapter; no worker launches after cancelled
or expired preparation. Six focused adapter tests passed again in combined-tree
validation. A running filesystem syscall cannot itself be cancelled, and final
authorization-to-exec remains a race. Additional routing and cleanup checkpoints
below describe the implemented slice; full PR6 acceptance remains outstanding.

## Current provider scope

Operator-selected migration scope is OpenAI and Anthropic for now. Further chat
provider migrations are deferred; the already implemented Ollama package remains
available. This narrows provider coverage, not a claim that every channel/media
or provider-operation requirement in the original PR6 plan is complete.

## Independent Anthropic package checkpoint

`crates/praxis-provider-anthropic` now contains the extracted adapter and an
authenticated one-shot worker. The native facade preserves Praxis attribution
and APIs. Checked private-pipe credential grants precede provider construction;
validated arguments never carry secrets. Compiled-worker fixtures cover bounded
JSON, model/system/tool/output-limit forwarding, usage, scoped grants and safe
errors without retries. Unsupported model-list/embedding operations are not
advertised. Package validation: 12 tests, strict all-targets Clippy; focused
core-only native validation: six branding and eight Anthropic tests. Main repeated
package tests and the full ordinary root suite: **1019 passed, 53 ignored**.
This addition is not full PR6 completion.

## Latest combined validation

The offline real-POML message-loop acceptance suites now pass: **24 core-only**
and **27 compatibility** tests, including verified coding-role transitions,
learning inbound-reply guards, task-local discovery, rollback/commit, compaction
failure preservation and non-reexecution. Fixtures isolate project roots from
asset roots and use owner-aware catalogs; optional shell tests remain optional.
The standalone tool loop now accepts an explicit host-selected workspace and
binds both action contracts and tool receipts to that root. The former hardcoded
`.` verification root caused a genuine standalone rollback/commit failure;
its core-only regression now passes.

Latest ordinary root suite: **1018 passed, 53 ignored**. Known temporary SM/skill
fixtures left by killed test workers were quarantined outside the repository
(`/tmp/pr6-test-artifacts-81ebdci2`) before rerunning; no tracked/operator assets
were removed. Route epoch tests remain separately covered by worker validation.
These checks still do not finish PR6's remaining operation/package and release
acceptance scope, and nothing has been pushed.

## Workflow acceptance checkpoint

The real-Cargo ignored verified-Rust workspace fixtures were explicitly executed:
both agent-loop and message-handler variants passed (2 tests). A real local POML
render check first reproduced dropped populated learner requests. The language
teacher persona now includes the current learner request when nonempty in both
legacy and package-owned copies; byte parity and all nine local render cases
passed (empty/missing/French/Japanese, chat and agent). No paid inference or
persistent deployment context was used. Installed asset-catalog and complete
Snake acceptance still need final combined validation.

After preparation edits settled, the deterministic core-only script passed
3 chat/task fixtures (28 ignored), 67 coding/contract fixtures (4 ignored), and
13 learning/memory fixtures. This does not establish full acceptance. Explicitly
running the offline real-POML message-loop fixtures without default features
exposed **15 passed, 10 failed**; failures include missing tools at continuation,
unexpected extra generation, absent temporary side effects and an apparently
lost compaction summary. Investigation isolated optional-tool fixture assumptions,
shared-workspace receipt invalidation, incomplete standalone catalog input, and
a positional compaction assertion. Compaction input was intact: trusted policy
insertion moved the user prompt from index zero. The assertion now selects the
user message and also checks policy; both default/core configurations passed,
including failure-history preservation. Remaining fixture fixes are in progress.
The script now labels its limited guard
scope honestly and includes real-pipe registry routing tests.

Selected contribution loader provenance is now recorded as host-only exact
manifest/package paths, rather than reconstructing locations from package names.
Its host-only provenance regression passed. The bounded, latched activation
primitive passed seven tests with and without default features; actual route
epoch integration is now implemented. The retained route validates selected disk
activation before launch and terminal success, cancels/reaps on observed changes,
suppresses later callbacks, and never resurrects an invalidated epoch after
reenabling or router rebuild. Worker validation reports 31 route and six adapter
tests passed. Observed invalidation plus reaping ranged from 3.6–111.6 ms in
fixtures; the 100 ms polling interval and two-second probe/cleanup budgets are
not instantaneous revocation or atomic executable attestation. Final combined
validation is still pending.

## Independent Ollama package checkpoint

`crates/praxis-provider-ollama` now contains the native adapter implementation and
an anonymous one-shot worker. The kernel facade preserves public APIs and Praxis
HTTP attribution. The independent normal dependency tree excludes kernel,
channel, VM and media packages. Worker fixtures cover bounded streaming/JSON,
URL/model/options forwarding, usage and safe errors without hidden retries;
a batched-stream overflow regression passes after yielding between events.
Embeddings/model-list operations are not advertised on the chat-only wire.
Package tests and strict Clippy passed; native validation reported 24 passed,
1 ignored, plus five branding tests. OpenAI/Ollama/shared-HTTP package tests also
passed again in combined-tree validation.

The combined-suite timing regression in
`registry_route_stream_telemetry_and_timeout_retry` is fixed with deterministic
injected typed timeout coverage; the separate real-deadline/reaping test remains.
Latest full root suite passed **994 tests, 52 ignored**. Preparation/chat six,
real-pipe routing seven and provenance one also passed in focused validation.
This excludes the failing explicitly invoked core-only POML fixtures above;
there is no release acceptance or push.

## Opt-in runtime routing and cleanup checkpoint

`PRAXIS_REGISTRY_CHAT_PROVIDER=<id>` explicitly adds a registry-backed chat route;
installation alone never activates it. Select the ID through `USE_PROVIDER` or
context. Native routing remains unchanged without opt-in. Login router rebuilds
retain the binding and quota gate. Seven real-pipe router tests passed again in
combined-tree validation, covering credentials, opaque continuation, streaming,
usage, deadlines/cancellation and no-launch rejection paths. Broader gateway
validation reported 386 passed, 40 ignored.

Unix provider workers now own an isolated process group. Readiness failure,
explicit termination and dropped futures/process ownership kill ordinary helper
descendants as well as the direct worker. Linux regression first reproduced a
surviving helper, then passed; a paired termination/drop regression checks helper
shutdown and direct-child reaping. Provider API validation: 65 tests passed,
all-targets Clippy clean. This is not a sandbox: descendants that deliberately
escape the group are outside containment; non-Unix platforms retain direct-child
cleanup only.

Asset ownership read/mutate/save is now serialized across cooperating processes
with a persistent private OS lock. Four focused lock tests passed again, including
actual subprocess apply/remove operations with distinct owners. This requires
Rust 1.89+ and does not provide cross-file transactions or hostile-rename safety.

Remaining routing guarantees include active disable/revoke/replacement
subscriptions (runtime still snapshots startup registry), preparation admission
across recreated adapters, and nonblocking route preflight metadata. Independent
channel/media worker packages and workflow/release acceptance remain incomplete.

## Asset filesystem safety checkpoint

Lifecycle asset handling preflights source, destination, record, and pruned paths;
rejects component symlinks, Unix hardlinks and path aliases; and publishes each
file atomically. Ownership hashes/operator edits are preserved, including failed
upgrade cleanup that previously could remove untracked files with matching bytes.
Ten dedicated safety tests and eighteen lifecycle tests passed in worker validation.
Combined validation: plugin tests 120 passed (4 ignored); root library tests
972 passed (52 ignored). Follow-up review fixed publication permissions: existing
modes/execute bits are preserved and new files keep private tempfile defaults.
Full ownership records now fail closed on malformed owner maps, aliased paths,
or invalid SHA256 hashes, without mutating asset bytes/records. Latest combined
root library validation: 975 passed (52 ignored). Cross-process ownership-map
read/modify/write serialization remains unresolved, as do cross-file transactions
and hostile concurrent directory renames; these are not rollback acceptance.

## Script credential isolation checkpoint

The v1 script launcher now clears ambient environment and restores an explicit
compatibility allowlist. Provider/admin tokens, proxy and loader overrides are
not inherited; declared credentials remain in host-constructed `PLUGIN_SECRETS`.
Subprocess regressions cover unrelated secret exclusion and declared grants
without mutating shared process environment. Worker validation: 118 plugin tests
passed (4 ignored), media Python tests 18 passed, ComfyUI Python tests 25 passed.
This is not filesystem/network isolation. MCP `env_from` now resolves only from
host-granted `PLUGIN_SECRETS` (no ambient credential fallback), rejects malformed
or duplicate-key envelopes, and preserves output redaction/safe PATH forwarding.
Operator migration is documented in the MCP package. All 21 MCP tests passed
again in combined-tree validation.

## Discord security checkpoint

Scoped Discord credentials now have redacted Debug output. File delivery uses
its startup-authenticated HTTP client rather than rereading ambient bot-token
environment variables; missing files fail before network access and raw delivery
errors are not logged. Closed delivery streams stop, while lagged streams recover.
Five new regressions include actual multipart authorization/content and shutdown.
`cargo test -p praxis-channel-discord`: 39 passed, doc-tests passed.
This does not establish an installed independent channel worker, delivery grants,
active revocation, or complete shutdown ownership.

## Remaining acceptance work

1. Provider process bridge and server lifecycle, authenticated request identities,
   narrow credential delivery, cancellation/death handling, ready/capability checks.
2. Separate independently buildable/installable provider worker packages, routing
   from host registry, streaming/model/embedding/decision support and opaque
   continuation ownership. Preserve host budget fitting, backoff/retry,
   compaction, vision, attribution and usage; no worker-side hidden retries.
3. External channel package event/delivery transport and complete
   enable/disable/uninstall/shutdown lifecycle, preserving host caller authority.
4. Independent voice/transcription/synthesis and media/GPU packages with explicit
   worker startup/secret/capability declarations, resource limits and host authority.
5. Asset/catalog defaults, migration/build/lockfile handling, package availability
   and missing/disabled/unready behavior without silent native fallback.
6. Cross-process integration tests: malformed/oversized streams, stale bindings,
   cancellation, credential isolation, restarts, active-session disabling,
   upgrade/rollback and functional parity for all providers/channels/media.
7. Full regression/review, release build, deployment and push only after acceptance.

## Checkpoint tests

`cargo test -p praxis-provider-api`: 53 passing unit tests.
Transport/handshake/process tests cover framing, stale readiness, strict decoding,
launch configuration and missing executable failures. Invalid identity and wire
limits are rejected before executable lookup/spawn, with regression coverage.
`WorkerLaunch::spawn_cancellable` rejects pre-cancelled launches without spawning
and kills/reaps workers cancelled during readiness; real-process cancellation
coverage is bounded by a three-second outer deadline. Cancellation remains forced,
not a graceful acknowledgement protocol. Unix OS-process tests cover
ready negotiation, cleared environment, forced termination/reaping, and death
during readiness (the Python fixture skips if `/usr/bin/python3` is unavailable).
One-shot exchanges connect checked request/stream codecs with deadlines and
forced cancellation; worker provider/generation/nonce binding is checked before
request delivery. Real-pipe exchange tests cover terminal success, EOF, invalid
sequence, deadlines, pre-cancellation, cancellation after a checked delta,
foreign provider/generation/nonce, and invalid deadlines. These exchange fixtures
require `/usr/bin/python3` on Unix. Cleanup failures cannot produce successful
exchange results; readiness-failure cleanup is bounded to two seconds.
Worker-side readiness now shares the bounded handshake protocol, rejects wrong
provider/version/capabilities and invalid correlation identities before emitting
Ready, and has paired host/worker pipe tests.
Worker-side one-shot request receipt checks negotiated provider/generation/nonce
and checked request invariants. Event writing enforces sequence, terminal and
stream limits; validation errors, transport failures and interrupted partial
writes poison the writer against retry or successful finish. Dedicated server
tests cover foreign identities, EOF, terminal/post-terminal handling, failed
transport and cancellation during a blocked partial write. A paired session test
exercises readiness, request receipt, terminal error delivery and clean EOF over
a 64-byte duplex pipe, under a two-second outer deadline. This is an in-process
protocol integration test, not independent-worker acceptance.
Rust fixture examples now exercise the same host launcher and worker-side APIs
across real OS pipes, without Python. Run `cargo build -p praxis-provider-api
--examples` followed by `target/debug/examples/fixture_host` (passed). The worker
uses a fake worker-local `ChatProvider` through `worker::execute`. Three real-pipe
checks verify ordered streaming success, safe provider errors, and queue overflow
without deltas or a successful terminal. No network/provider operation is performed;
the fixture is never installed or routed as a native fallback. These examples are test scaffolding,
not independently packaged production providers.
`bridge::invoke` now provides a single host entry point: validates request and
launch binding before spawning, shares a deadline/cancellation token across
readiness and exchange, and never retries or falls back. The deadline accepts
0 < duration <= 300 seconds; forced cleanup may add up to two seconds. Readiness
timeout/cancellation tests use actual OS processes; the Rust fixture host now
uses this entry point. Automated real-process invocation tests cover checked
terminal success, missing-terminal EOF, and a foreign request-ID delta rejected
without invoking the callback. The old speculative bridge tests were replaced with
registered tests for the implemented API. This does not yet implement the
`ChatProvider` runtime adapter or registry routing.
`worker::execute` connects a received checked request to one worker-local
`ChatProvider::chat_attempt`. Legacy continuation decoding remains inside the
worker. Delta buffering is bounded to 16 items; overflow fails closed, and queued
deltas precede the checked terminal attempt or safe provider error. Tests cover
ordered deltas, success/error terminals, exactly one attempt and overflow without
a successful terminal. Interruption tests also verify the active provider future
is dropped and the owned pipe closes without a terminal; foreign-bound or
unknown-format legacy continuations are rejected before calling the provider. Callers must bound this future and close the pipe after
failure/cancellation. This is adapter infrastructure, not a production worker
package or a host routing switch.
`worker::serve_once` owns the private streams and applies one session deadline
(positive, at most 300 seconds) and cancellation token across readiness, request
receipt and execution. Cancellation drops the session without forging a terminal;
provider construction, credentials and process exit remain caller-owned. The Rust
fixture now uses this API. Tests cover paired session success, pre-cancellation,
readiness timeout and invalid budgets without calling the provider.
Graceful cancellation, secret provisioning and routing acceptance remain pending.
`cargo clippy -p praxis-provider-api --all-targets -- -D warnings`: passed.
`PluginRegistry::prepare_chat_worker` maps an explicitly selected enabled provider
contribution to a secret-free launch configuration without spawning. Each call
reloads operator trust and validates ownership/resolution, arguments, correlation
identity and deadline. Credential-requiring declarations and unsupported
model/embedding/decision operations fail closed. This is preparation only: no
routing switch, launch-time content attestation or atomic lifecycle grant; callers
must prepare again for each invocation. Tests cover current/revoked/downgraded
trust, missing/disabled providers, unavailable files and incompatible declarations.
`PluginRegistry::invoke_chat_worker` now connects per-call preparation to the
checked bridge for an explicitly host-selected request. It caps the invocation
budget to the declaration timeout, preserves typed bridge errors through anyhow,
keeps continuation opaque, and never retries or falls back. Unix OS-process tests
verify invalid/pre-cancelled/untrusted calls do not spawn, worker death preserves
a safe readiness error, and a 60-second caller budget is capped by a one-second
contribution timeout with exactly one launch. Existing automatic routing remains
unchanged; active disable/trust-change subscription is still absent.
`cargo test --lib contribution_tests`: 9 passing unit tests.
Preparation now consumes the same invocation budget; an already-spent budget
returns timeout before spawning (covered by the registry invocation test).

OpenAI request formatting/HTTP behavior now lives in the independent
`praxis-provider-openai` crate. Its compiled anonymous-endpoint worker serves the
checked one-shot protocol without linking the kernel. Three process integration
tests cover checked terminal response, model/output-limit forwarding, safe 429
retry metadata with no hidden retry, and refusal of secret-bearing startup URLs
or extra credential arguments. Authenticated worker delivery was added below;
secret-store authorization and runtime routing remain pending.
The native facade deliberately remains until automatic routing migration is done;
its root-branded HTTP client preserves existing behavior (a branding regression
was caught and fixed by root tests).

Shared HTTP handling now lives in `praxis-provider-http`, with a compatibility
facade for other native adapters. Successful JSON bodies are bounded to 16 MiB
before aggregation/parsing; error-body inspection remains capped at 8192 bytes
and one second. HTTP tests moved out of the root, explaining the root count drop.

Credential transport foundation now lives in provider API `credentials`: explicit
host-declared grants bind provider/generation/nonce, cap total frames at 64 KiB,
allow at most 32 names (128 bytes each) and 4096 bytes per nonempty value, and
reject undeclared keys, duplicates, unknown fields, invalid versions and foreign
recipients. Debug/errors redact values. Five tests cover private-pipe round trips,
identity/declaration failures, no-write foreign sends, malformed and oversized
frames. The codec is now integrated into `worker::serve_once_with_credentials` and
`bridge::invoke_with_credentials`: recipient/size validation happens before
spawn, grants follow readiness and precede requests, and grant I/O shares the
invocation deadline/cancellation and forced cleanup path. Provider construction
occurs only after a checked grant. Three new tests cover session success/foreign
and undeclared grants/missing-grant timeout, no-spawn foreign grants, and a real
OS process checking credentials are absent from arguments/environments.

The compiled OpenAI worker now supports the non-secret `--credentials` flag and
requires a declared `api_key` grant in that mode. It rejects missing/invalid
header values before HTTP; another process integration test proves bearer auth
and rejection of a control-character key without another HTTP request. Anonymous
mode remains compatible. Explicit registry secret-store authorization is now provided by
`prepare_chat_worker_with_secrets` and `invoke_chat_worker_with_secrets`: current
runtime trust and unique enabled ownership are rechecked, only the contribution's
declared names are resolved via the existing plugin-secret policy, missing or
placeholder credentials fail closed, and no whole store/native-key remapping is
sent. The anonymous API still rejects secret-requiring declarations. A focused
test covers scope, redaction, missing/placeholders and trust revocation; all 10
registry contribution tests pass. A subsequent real OS-process registry test
also proves grant delivery, exclusion of unrelated secrets/arguments/environments,
no spawn on untrusted/missing/placeholder/oversized/invalid/cancelled/spent/zero
calls, checked terminal success, and cancellation after the child has checked its
grant without retry. Latest root validation: 958 passed, 52 ignored (11 focused
contribution tests). Automatic routing and active revocation subscriptions are
NOT implemented.
Cancellation or failure during grant send requires disposing of the pipe, not
retrying a partial frame. No memory-zeroization guarantee is claimed.

Latest validation: provider API 61 tests, provider HTTP 3 tests, compiled OpenAI
worker 4 integration tests; Clippy for all three packages/all targets with
`-D warnings` clean. `cargo test --lib`: 956 passed, 52 ignored, 0 failures
(existing compiler warnings remain). No final PR6 acceptance or git push yet.
A focused independent review was completed and its registration/digest/bounded
conversion findings were addressed with regression tests.

Host chat API now carries `ProviderContinuation::Worker { state }` without
parsing worker payloads. `WireContinuation::binding` exposes correlation metadata
only; size-based token estimation preserves host budgeting without interpretation.
Conversion back to the wire validates the original provider/generation/call and
current continuation byte budget, rather than rebinding opaque state. Two tests
cover arbitrary non-JSON payloads, redaction, foreign binding and smaller budgets.
The legacy worker decoder still rejects unknown formats; native Codex does not
forward worker state. This is a prerequisite for, not the completed host runtime
adapter or routing switch. Validation: provider API 63 tests, API all-targets
Clippy clean; root 958 passed, 52 ignored.

An opt-in `plugins::registry_chat::RegistryChatProvider` now implements the host
`ChatProvider` trait using the checked bridge and current registry/secret store.
It reauthorizes every attempt, pins the selected package owner, releases locks
before async execution, preserves checked streaming and opaque continuation,
uses fresh request IDs/nonces while retaining continuation call binding, and
maps safe provider rate-limit/status metadata without hidden retries/fallback.
Budgets include synchronous preparation and cap to the declaration timeout.
Unsupported legacy/foreign continuations and unresolved vision routing directives
fail before spawn. Model listing is unsupported and health is not claimed.
The adapter now pins the declaration and bounded implementation digest at
construction and rejects changed revisions before subsequent launches. A real
process regression proves an upgraded executable is not invoked by the old
adapter. This is an invocation-time upgrade check, not race-free exec attestation.
The caller must still supply a new logical generation/adapter after upgrades;
launch-time byte attestation and active disable/revocation remain unresolved.
Nested exchange timeout/cancellation now retain their typed host errors (TDD
regression), rather than becoming generic transport failures.
A real-process test covers streaming callbacks without registry locks, arbitrary
opaque continuation round trips, fresh identities, revoked trust/disabled owners,
foreign generation and cancellation. Two unit tests cover safe error mapping.
Latest root validation: 961 passed, 52 ignored. Automatic routing and release
installation remain unchanged; this adapter is not full provider parity acceptance.

Focused tests do not establish PR6 acceptance or full runtime compatibility.
