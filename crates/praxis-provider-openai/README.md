# OpenAI-compatible provider package

Build independently with `cargo build -p praxis-provider-openai`. No dependency
on the Praxis kernel, database, router, channel or media implementations.

The executable accepts non-secret configuration arguments:
`BASE_URL MODEL [--credentials]`. It serves one bounded `praxis.provider.v1` chat attempt on
launcher-owned stdin/stdout pipes, with stdout reserved for protocol frames.
It advertises output-limit support, but not model enumeration or embeddings.
Retries, cancellation, budgets, tool execution and fallback remain host-owned.

Without `--credentials`, endpoints remain anonymous. With the flag, the worker
requires an identity-bound private-pipe grant containing only the declared
`api_key` before accepting the request; the provider is constructed afterward.
Keys must be nonempty printable ASCII without whitespace or control characters.
Credentials in arguments, URL userinfo, query strings or fragments are rejected.
Do not put API keys in package arguments or environments. Explicit registry invocation can authorize the declared names from a host
secret store; automatic host routing is not implemented yet. The `api_key` name
must be explicitly present in that store's plugin-secret policy; there is no
implicit mapping from native OpenAI account credentials. The library retains the native constructor for the
transitional compatibility facade; library availability is not worker activation.

A prepared contribution may declare:

```json
{
  "id": "openai",
  "executable": "bin/praxis-provider-openai",
  "protocol": "praxis.provider.v1",
  "args": ["http://127.0.0.1:8080/v1", "local-model"],
  "capabilities": ["chat", "output_limit"],
  "secrets": [],
  "timeout_secs": 120
}
```

Place it under `provides.providers` in a runtime-role package after installing
the compiled binary inside the package. Operator runtime trust is required;
no tool declaration or tool-trust grant substitutes for provider ownership.

Tests run the compiled executable against a local HTTP fixture through the real
host bridge, checking correlation/readiness, exactly one HTTP request, model and
output-limit forwarding, terminal response, safe rate-limit metadata and invalid
startup configurations, authenticated private-pipe delivery and invalid header
values rejected without another HTTP request. This is not full PR6
runtime/release acceptance.
