# praxis-provider-ollama

Independent Ollama HTTP adapter and anonymous one-shot worker. Normal dependencies
are only the provider API/HTTP packages and transport utilities: no Praxis kernel,
channels, VM, database or media stack.

```sh
cargo test -p praxis-provider-ollama
cargo clippy -p praxis-provider-ollama --all-targets -- -D warnings
cargo build -p praxis-provider-ollama --bin praxis-provider-ollama
praxis-provider-ollama BASE_URL MODEL
```

The executable uses the provider API's private-pipe `serve_once` protocol, not a
JSON CLI. It accepts exactly two configuration arguments, rejects URL userinfo,
queries/fragments and control characters, and reads no credential environment
variables or credential arguments. Stdout is protocol-only; failure diagnostics
are static. Authenticated Ollama remains supported by the native Rust constructor,
not this anonymous worker.

URLs retain the configured prefix and trim trailing slashes before appending
`/api/chat` or `/api/tags`. Chat streams NDJSON through checked wire deltas, including
usage, reasoning/tool previews, output-limit errors and host-owned continuations.
The adapter does not retry: a chat attempt can make one cached best-effort tags
probe (five-second timeout) and one chat POST. Batched HTTP chunks yield between
deltas so the one-shot executor can drain its bounded event queue.

Shared HTTP helpers disable reqwest retries, sanitize HTTP failures and bound
model-list/probe JSON. Chat JSON and cumulative NDJSON use the same shared 16 MiB
limit; a local bounded JSON reader preserves Ollama's scalar in-band error
classification, which the generic helper cannot express. Missing final stream
records fail rather than forging success.

The native facade supplies the Praxis attribution client and retains `new`,
`num_ctx`, `name`, `as_any`, streaming, health and model-list APIs. Context reduction
is extracted locally, including durable tool-output handles and the mandatory
safe-web-search suffix; it does not depend on the kernel policy modules.

## Limits of this PR6 slice

This is a package/worker extraction, not full PR6 integration. Registry routing,
plugin installation and authenticated worker grants are outside this slice.
The wire only supports chat attempts, so embeddings and model-list capabilities
are **not advertised** (native model listing remains available). No live Ollama
server is required by the tests. Existing native image-path loading semantics
are preserved; this worker is not a filesystem sandbox.
