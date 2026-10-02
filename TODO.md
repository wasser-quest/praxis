# TODO — Praxis Local Model Reliability

## Completed ✅
- [x] Real Praxis pipeline (Gateway → POML → history/tools → llama.cpp/pgpu)
- [x] Native SSE streaming + thinking display (`/show_thinking`, Discord codeboxes)
- [x] `/stop` command, session renaming (TUI/Web)
- [x] 500-task benchmark (292/500 running)
- [x] Decision endpoint activated (qwen3-1.7B, `--decision-seqs 3`)
- [x] Configurable tag prefix (`settings.tag_prefix`)
- [x] Free router integration (`settings.use_freerouter`)
- [x] Decision routing (pre-request, CAS commit, failure-safe)
- [x] Four-arm experiment runner and offline evidence-scoring validation (live comparison remains open)
- [x] Ollama chat/System One integration with model selection and probability-threshold routing (live comparison remains open)
- [x] Context compaction before requests (shared service)

## In Progress 🔄
- [ ] Recover and validate the historical 5/20-task checkpoint and its original runtime snapshot
- [ ] Complete the paired 20-task × 4-arm live comparison; keep historical and v2 measurements separate
- [ ] Review quality, policy compliance and cost against fixed/entry/continuous before claiming Decision benefit

### Verified Execution — next sequence
- [x] Named-check receipts, guarded transitions/completion and transactional host-file patches
- [x] Optional plugin action/effect contracts, independent pre/post verification, bounded execution and compensation
- [x] Task-owned action receipts and `.sm` action receipt guards
- [x] Opt-in semantic Rust workspace-tests capability (verified-capabilities example)
- [ ] General runtime-verified facts/receipts in `.sm` transition guards
- [ ] Semantic capabilities replacing raw terminal orchestration gradually
- [ ] Compact Praxis Decision IR after execution semantics and evaluation evidence

Protocol and current evidence: [Decision-router experiment](docs/DECISION_ROUTER_EXPERIMENT.md).

## Not Started ❌
### Trusted-Template Lifecycle Actions
- [ ] `PREFIX/clearmessagesholdimportantones` — classify older content good/bad for relevance
- [ ] `PREFIX/forgetallthistemplaterememberallafterthat` — template/state context boundary
- [ ] Trusted-template authorization (not user/model text)
- [ ] Per-message good/bad labels
- [ ] Whole-turn/tool-group protection
- [ ] Template-entry watermark boundary
- [ ] Atomic apply/rollback, cancellation, audit/restore

### OpenAPI/API Explorer + Scoped/Expiring/Revocable API Keys
- [ ] OpenAPI spec generation (Praxis + pgpu)
- [ ] API explorer UI in dashboard
- [ ] Key hashing, expiry, revocation, resource scopes
- [ ] Server-side checks for indirect access (chat→tools→WS→proxy)
- [ ] No self-escalation by delegated keys
- [ ] pgpu integration (separate repo)

### Deployment & Validation
- [ ] Deploy updated Praxis binary
- [ ] Live Discord/TUI end-to-end validation
- [ ] Compaction non-default session atomicity audit
- [ ] Oversized-template fallback validation
- [ ] Archive artifacts outside `/tmp`

## Notes
- Historical Decision server: `http://100.105.88.246:11440` (qwen3-1.7B)
- Historical chat model: `http://100.105.88.246:11435` (qwen3.8-27B)
- Historical pgpu router: `http://100.105.88.246:8080` (slot 1)
- Solver/Decision reachability from the current workspace failed on 2026-10-02; historical healthy status is not current validation.
- Benchmark artifacts: `/tmp/praxis-live-baseline-9uao6aup/`
- State experiment: `/tmp/praxis-live-baseline-9uao6aup/state-experiment-4arm/`
