# Context variables — complete reference

This reference covers all **11 `Context` fields and 86 `ContextSettings` fields** in `src/db/contexts.rs`, plus the built-in POML/runtime variables. It describes the implemented behavior, not just dashboard labels. See [Discord voice setup](DISCORD_VOICE_SETUP.md) for microphone → STT → reply → TTS → Discord playback.

## Storage, types, defaults, and safe editing

The active deployment is `/workspace/release`. Its context is stored in `data/praxis.db`, in the `contexts` table's JSON `data` column. The `contexts/` and `contextlanguage/` directories contain optional workflow/preset files; simply editing a JSON file there does **not** update a saved database context. Workflows run before rendering on both message paths. Select them through `sm_file` (or the higher-priority `settings.sm_file`); the default is `standard`. Legacy `cl_file` and `cl_data` inputs are normalized to `sm_file` and `sm_data`. Old saved workflow data is preserved; normal saves/output use only the canonical names.

Use the dashboard context editor or `/context` rather than editing SQLite while Praxis is running. In Discord, select `/context` and put the following in its **command** argument. The web chat/TUI accept the complete slash command:

```text
/context show settings
/context get settings.voice_wake_words
/context set settings.voice_stt_type=elevenlabs settings.use_stt=true
/context set settings.system_template=language_instructor
/context set settings.allowed_channels=["123456789012345678"]
```

- Use the full `settings.` prefix. `custom_data.mode` is **not** the real top-level `mode`.
- JSON booleans are `true`/`false`, not strings or `1`/`0`. Arrays must be JSON arrays. Quote numeric Discord IDs because these fields require strings.
- A dotted update changes only that leaf. Dashboard and `/context` updates recursively merge JSON objects; arrays/scalars replace the addressed value. Prefer precise dotted updates, and back up before broad changes.
- `null` is allowed only for optional fields or arbitrary JSON. `/context unset` writes `null`; it does not remove the key or reset a non-optional field to its default.
- Unknown top-level and `settings` keys are discarded during Rust deserialization. Put extension values in `custom_data` or `sm_data`, not arbitrary root/settings keys.
- **Defaults below are for newly created contexts.** Defaults apply when fields are absent, not when an explicit value is present. Four partial-JSON exceptions are listed below.
- **Allowed values:** Rust enforces JSON types and integer representability. Most strings and numeric settings have no additional central validation. A listed operational choice/range can therefore be required by the consumer even if the editor accepts other values. Provider-specific models, voices, languages, and credentials are validated by that provider, not by saving the context.
- `I32` means integer **−2,147,483,648 through 2,147,483,647**. `Usize` means integer **0 through 18,446,744,073,709,551,615 on this 64-bit deployment**; use practical small values. JSON numbers passed through a browser may lose precision above 2^53−1.
- `F64` means a finite representable 64-bit floating-point number.
- `F32` means a finite representable 32-bit floating-point number. JSON does not support NaN or infinity. ElevenLabs ranges below are also checked before sending TTS requests.
- Relative filesystem paths resolve from Praxis's working directory. Run the deployed binary from `/workspace/release`. Never put API keys, passwords, or other secrets into context/POML: context can be sent to the LLM and included in logs.

### Partial-JSON default exceptions

If an existing JSON `settings` object omits a field, Serde currently uses these values instead of `ContextSettings::default()`:

| Field | New context | Field absent from an existing settings object |
|---|---|---|
| `history_with_toolcalls` | `true` | `false` |
| `agent_name` | `"assistant"` | `""` |
| `voice_wake_words` | `["*"]` | `[]` — no transcript is forwarded |
| `elevenlabs_speed` | `0.8` | `null` — provider default |

If the entire `settings` object is absent, the new-context defaults are used instead. Existing explicit values are preserved.

## Root context fields

| Variable | Type | Allowed values / operational constraints | Default | Effect |
|---|---|---|---|---|
| `user_id` | string | Existing internal user/context ID, normally a UUID; required in stored JSON | `""` before allocation | Identity used for messages, pairing, and storage. Do not rename it in place. This is not the Discord numeric user ID. |
| `turn` | I32 | Representable integer; use ≥0 | `0` | Message-turn counter maintained by the gateway; not an LLM token count. |
| `mode` | string | Operational values `"agent"`, `"chat"`; other strings are stored but have no defined mode semantics | `"agent"` | Passed to prompts. It does not by itself select the multi-turn path or reliably disable tools; `max_llm_turns` selects the path. |
| `username` | string or null | Display text or `null` | `null` | Display name; runtime POML normally falls back to `"User"`. Not a credential or identity key. |
| `sm_file` | string or null | Relative workflow name under `contexts/`, or `null`; `.sm`/`.cl` extension may be omitted | `null` → `standard` | Fallback workflow selection if `settings.sm_file` is null. Runs before rendering on both runtime paths. Legacy `cl_file` is accepted on input, never emitted. |
| `active_state` | string or null | State name defined by the selected workflow, or `null` | `null` | Root workflow state used by the SM engine and dashboard. Distinct from the duplicate field under settings. |
| `active_templates` | array of strings | Template names, e.g. `["tasks/plan"]`; `[]` allowed | `[]` | Root list exposed by dashboard workflow endpoints. Not the same as the settings template stack; neither replaces `settings.system_template`. |
| `settings` | object | The 86 documented settings below; do not set to null | New-context defaults | Typed per-context configuration. Prefer dotted updates to individual fields. |
| `custom_data` | JSON | Prefer an object, or `null`; arbitrary extension keys/JSON values | `null` | Application/plugin data and tool history. Exposed to POML; null is normalized to `{}` for system rendering. Non-object values can break consumers. |
| `sm_data` | JSON | Prefer an object, or `null`; arbitrary workflow-specific JSON | `null` | Canonical statemachine data namespace; supports dotted updates and POML. Legacy `cl_data`/`cl_data.*` inputs are accepted; canonical values win conflicts. |
| `session_id` | string | `""` or `"default"`, or a session ID created through session commands; avoid the reserved `:::` separator | `""` | Selects the session. Non-default rows use `user_id:::session_id`; the base row points to the current session. Use session commands rather than manually editing IDs. |

## Voice: input, output, and Discord

The working Discord STT gate is **`voice_enabled && use_stt`**, plus a joined, non-deafened bot and a paired speaker. The normal **Discord** reply-TTS gate is **`use_tts`**, not `voice_tts_enabled`. A `voice:` input also enables reply TTS even when `use_tts` is false.

**Web dashboard speech is independent:** `settings.web_chat_tts` (boolean, default `false`) controls web-reply synthesis and automatic browser playback. Disable it for the active context with `/context set settings.web_chat_tts=false` or the chat speaker button. `use_tts=true` and explicit TTS feedback cannot override this web OFF setting. Context edits update connected dashboards immediately and cancel current/queued autoplay; reconnects and new automatic clips recheck the setting. Discord side-channel autoplay requires both the active chat and the source context to enable `web_chat_tts`. Saved audio remains available through manual replay buttons, without another provider call.

| Variable | Type | Allowed values / operational constraints | Default | Effect |
|---|---|---|---|---|
| `settings.voice_enabled` | boolean | `true`, `false` | `false` | Enables the Discord transcription pipeline together with `use_stt`. Does not join a channel or enable TTS on its own. |
| `settings.voice_stt_type` | string | Exact lowercase `"elevenlabs"`, `"vosk"`, `"whisper"` | `"vosk"` | STT engine. ElevenLabs and remote Vosk work without local speech models. Local Vosk needs the `voice_vosk` feature/model; set `voice_vosk_url` for remote recognition. Whisper remains a stub. |
| `settings.voice_vosk_model_path` | string or null | Readable local Vosk model directory, or `null` | `null` | Used only when no remote Vosk URL is configured. Local Vosk needs the build feature/library/model; only one local model is cached process-wide. |
| `settings.voice_vosk_url` | string or null | Standard Vosk `ws://IP:2700` or `wss://host/path` URL without credentials/query/fragment; `null`/empty inherits environment | `null` → `VOSK_SERVER_URL` → local Vosk | With `voice_stt_type="vosk"`, sends PCM to the remote server without local Vosk libraries/models or build feature. No local fallback on remote failure. See [remote Vosk](VOSK_REMOTE.md). |
| `settings.voice_whisper_model_path` | string or null | Local model path, or `null` | `null` | Stored/passed to the Whisper branch, but cannot make the current stub transcribe. |
| `settings.voice_last_input` | string or null | Transcript text or `null` | `null` | **Stored only:** the current Discord STT pipeline does not update/read this field. Actual transcripts go through the message pipeline. |
| `settings.voice_listen_timeout_secs` | I32 | Representable integer; positive seconds recommended | `120` | **Stored only:** not the active silence timeout. Does not limit recording or HTTP request duration. |
| `settings.voice_owner_id` | string or null | Intended Discord user ID string or `null` | `null` | **Stored only:** not an enforced microphone-owner restriction. Voice routing uses SSRC mappings and pairing records. |
| `settings.voice_tts_enabled` | boolean | `true`, `false` | `false` | **Legacy/stored flag:** setting it alone does not enable spoken replies. Use `settings.use_tts`. |
| `settings.voice_tts_type` | string | Exact values `"elevenlabs"`, `"windows_sapi"`, `"minimax"`, `"mimo_tts"`, `"qwen_tts"`, `"comfyui_xtts"`, `"comfyui_qwen3"` | `"elevenlabs"` | TTS backend. Windows SAPI cannot work on Linux. ComfyUI uses the configured private XTTS workflow and existing playback; see [ComfyUI TTS](COMFYUI.md). Other backends retain their service/key requirements. |
| `settings.comfyui_base_url` | string or null | Private literal HTTP(S) origin, e.g. `"http://100.80.1.2:8188"`; null/empty inherits | `null` → legacy custom_data → `COMFYUI_BASE_URL` | Required for `comfyui_xtts` / `comfyui_qwen3`; use the GPU NetBird IP, not a public endpoint. |
| `settings.comfyui_tts_workflow` | string or null | Local API-format workflow path; null/empty inherits | `null` → legacy custom_data → `COMFYUI_TTS_WORKFLOW` → provider default | Local Praxis file, not GUI JSON. XTTS defaults to `workflows/tts-api.json` / node 1 `PraxisXTTS`; Qwen3 to `workflows/tts-qwen3-api.json` / node 1 `PraxisQwen3TTS`. Explicit old paths are not silently replaced. |
| `settings.comfyui_tts_language_mode` | string or null | `"single"` or `"de-ja"`; null/empty inherits | `null` → legacy custom_data → `COMFYUI_TTS_LANGUAGE_MODE` → `"single"` | Shared XTTS/Qwen3 mode. `de-ja` pronounces Japanese-script segments in Japanese and other text in German, then combines them into one WAV. Requires updated server nodes. See [mixed speech](COMFYUI_QWEN3.md). |
| `settings.comfyui_qwen_reference_audio` | string or null | Relative server-input audio filename; null/empty inherits | `null` → legacy custom_data → `COMFYUI_QWEN_REFERENCE_AUDIO` → `"reference.wav"` | Authorized Qwen3 reference on the GPU server; not a local Praxis path. |
| `settings.comfyui_qwen_reference_text` | string or null | Exact reference transcript, max 5000 characters; null/empty inherits | `null` → legacy custom_data → `COMFYUI_QWEN_REFERENCE_TEXT` → empty | Optional Qwen3 cloning transcript. Empty uses speaker embedding only; a correct transcript may improve similarity. Not a style instruction. |
| `settings.comfyui_qwen_language` | string or null | `"auto"`, `"en"`, `"zh"`, `"ja"`, `"ko"`, `"de"`, `"fr"`, `"ru"`, `"pt"`, `"es"`, `"it"`; null/empty inherits | `null` → legacy custom_data → `COMFYUI_QWEN_LANGUAGE` → `"auto"` | Qwen3 pronunciation language in `single` mode; `de-ja` mode overrides it without changing the saved value. |
| `settings.comfyui_xtts_reference_audio` | string or null | Relative server-input audio filename; null/empty inherits | `null` → legacy custom_data → `COMFYUI_XTTS_REFERENCE_AUDIO` → `"reference.wav"` | Pre-provisioned reference audio on the ComfyUI server, never a local Praxis path. |
| `settings.comfyui_xtts_language` | string or null | XTTS language code, e.g. `"en"`, `"de"`, `"es"`, `"fr"`; null/empty inherits | `null` → legacy custom_data → `COMFYUI_XTTS_LANGUAGE` → `"en"` | Per-user speech pronunciation language, not translation. |
| `settings.comfyui_timeout_seconds` | Usize or null | Integer 1–3600; null inherits | `null` → legacy custom_data → `COMFYUI_TIMEOUT_SECONDS` → `900` | Total ComfyUI submission, queue, execution and download budget. No automatic retry or shared queue interruption. |
| `settings.voice_elevenlabs_voice_id` | string or null | Non-empty voice ID available to your ElevenLabs account; `null` means unconfigured | `null` | Required for ElevenLabs TTS, not STT. A voice's display name is not its ID. |
| `settings.use_tts` | boolean | `true`, `false` | `false` | Enables normal non-web reply TTS; Discord `/tts` toggles it. Voice inputs also enable final/eligible feedback TTS independently of this flag. Does not override `web_chat_tts=false` for web input. Audio is broadcast to the active Discord playback channel. |
| `settings.web_chat_tts` | boolean | `true`, `false` | `false` | Independent web-reply synthesis and browser autoplay permission. False also blocks explicit web TTS feedback, without disabling Discord use_tts/voice replies or manual replay. |
| `settings.stt_low_confidence_threshold` | F64 | Finite number; practical confidence range 0–1 | `0.70` | Agent voice input marks custom_data.stt_low_confidence when the latest STT confidence is below this threshold; informs transcript clarification. |
| `settings.use_stt` | boolean | `true`, `false` | `true` | Enables STT together with `voice_enabled`; false skips provider transcription. |
| `settings.voice_muted` | boolean | `true`, `false` | `false` | **Stored only:** not synchronized with the handler's separate runtime mute state. `/mute` and `/unmute` currently control a runtime transcription gate, not this field or the TTS queue. |
| `settings.voice_deafened` | boolean | `true`, `false` | `true` | Applied to the actual Songbird call when `/join` runs. Set false and rejoin to receive audio. Also check Discord server/self-deafen controls; changing this stored value alone is not a live call update. |
| `settings.voice_discord_guild_id` | string or null | Intended Discord guild ID string or `null` | `null` | **Stored only:** playback uses runtime voice state established by `/join`, not this field. |
| `settings.voice_wake_words` | array of strings | `["*"]` always forwards; `[]` never forwards; otherwise non-empty trigger strings, e.g. `["Praxis"]` | `["*"]` | Case-insensitive substring matching **after STT**. Matching words are removed; specific-word matches currently lowercase the remaining transcript. Wake-word filtering does not prevent transcription charges. Avoid an empty-string trigger. |
| `settings.voice_auto_pause_enabled` | boolean | `true`, `false` | `false` | Captured on `/join`. True adds flushing after 1.5 seconds without content or 3 seconds of buffered audio. False flushes when a speaker is no longer present in a voice tick. Rejoin after changing. |
| `settings.voice_audio_output_path` | string or null | Writable local directory, or `null` for no saved copies | `null` | Saves generated/RVC/final audio under stage directories and `tts_output/`. Current file helper labels files `.wav` even if the provider returned MP3; inspect/decode the actual format. Not required for Discord playback. |

## ElevenLabs settings

Both STT and TTS use **Secrets → `elevenlabs_api_key`**. This is a secret, not a context variable. The legacy secret fields `voice_elevenlabs_api_key` and `voice_elevenlabs_stt_api_key` are not read by the current Discord path. Save encrypted changes with the master password and restart; voice/gateway components retain startup secret copies.

| Variable | Type | Allowed values / operational constraints | Default | Effect |
|---|---|---|---|---|
| `settings.elevenlabs_stt_model` | string | Non-empty ElevenLabs STT model ID; recommended/current `"scribe_v2"`; `"scribe_v1"` only if supported by your account and selected options | `"scribe_v2"` | Sent as multipart `model_id` to `/v1/speech-to-text`. Not a TTS model ID. Provider validates availability/options. |
| `settings.elevenlabs_stt_language` | string or null | `null`/`""` = automatic detection; otherwise a provider-supported ISO language code, e.g. `"fr"`, `"ja"`, `"de"` | `null` | Sent as `language_code` only when non-empty. Automatic detection is appropriate for French/Japanese/German practice. An unsupported code can be rejected remotely. |
| `settings.elevenlabs_stt_tag_audio_events` | boolean | `true`, `false` | `false` | Sent as multipart `tag_audio_events`; enables non-speech event tags when supported. Usually false for tutoring. |
| `settings.elevenlabs_stt_no_verbatim` | boolean | `true`, `false`; model must support the option | `true` | Sent as `no_verbatim`. Provider-controlled transcript cleanup; false is preferable if filler words/verbatim detail matter. |
| `settings.elevenlabs_tts_model` | string | Non-empty ElevenLabs TTS model ID; `"eleven_multilingual_v2"` is the multilingual baseline; alternatives require provider/voice support | `"eleven_multilingual_v2"` | Sent as TTS `model_id`. Supported voices, language overrides, and setting restrictions may differ by model. |
| `settings.elevenlabs_stability` | F32 | **0.0–1.0 inclusive**, checked before request; individual models may impose stricter choices | `0.5` | Voice stability. Zero is a valid value and is no longer silently replaced with the default. |
| `settings.elevenlabs_similarity_boost` | F32 | **0.0–1.0 inclusive**, checked before request | `0.75` | Similarity to the selected voice. Zero is preserved. |
| `settings.elevenlabs_style` | F32 or null | `null` = omit; otherwise **0.0–1.0 inclusive**, checked before request | `null` | Optional style/exaggeration setting; depends on the selected model/voice. |
| `settings.elevenlabs_speed` | F32 or null | `null` = provider default; otherwise **0.7–1.2 inclusive**, checked before request | `0.8` | Speech speed multiplier. Slower than 1.0 is useful for practice. See the partial-JSON default exception above. |
| `settings.elevenlabs_tts_language` | string or null | `null`/`""` = no forced language; otherwise a model-supported language code, e.g. `"fr"`, `"ja"`, `"de"` | `null` | Sent as `language_code` only when non-empty. Does not translate text. Leave null for mixed-language tutoring unless deliberately forcing one language. |

TTS explicitly requests `mp3_44100_128` and Discord decodes it before playback. Incoming Discord audio is decoded to 16 kHz mono PCM and wrapped in a WAV for STT. ElevenLabs HTTP requests have a 120-second timeout; this is not controlled by `voice_listen_timeout_secs`. Saving configuration does not prove key permissions, quota, model availability, or successful Discord playback.

## Optional TTS/RVC backends

These settings do not affect the selected ElevenLabs backend unless RVC is enabled. Paths and URLs must refer to services/files you control; do not insert credentials in URLs.

| Variable | Type | Allowed values / operational constraints | Default | Effect |
|---|---|---|---|---|
| `settings.rvc_on` | boolean | `true`, `false` | `false` | Enables post-TTS voice conversion. False avoids another service dependency. Missing RVC configuration is skipped; a conversion failure prevents final playback. |
| `settings.rvc_server` | string or null | RVC server base HTTP(S) URL without `/convert`, or `null` | `null` | Client appends `/convert`. Only used with RVC enabled and all required paths present. |
| `settings.rvc_model_path` | string or null | Model path understood by the RVC server, or `null` | `null` | Required for configured RVC conversion; not necessarily a path on the Praxis host. |
| `settings.rvc_index_path` | string or null | Index path understood by the RVC server, or `null` | `null` | Required by the current RVC readiness check. |
| `settings.qwen_tts_server` | string or null | Compatible Qwen TTS HTTP(S) server base URL, or `null` | `null` | Required for `voice_tts_type="qwen_tts"`; does not install or start the server. |
| `settings.qwen_tts_model` | string or null | Model label or `null` | `null` | **Currently no effect:** copied by the gateway but not passed to the Qwen client. Configure the model on the server. |
| `settings.qwen_tts_speaker` | string or null | Speaker ID supported by your Qwen server, or `null` for its default | `null` | Speaker selection in ordinary Qwen synthesis. |
| `settings.qwen_tts_language` | string or null | Language name supported by your Qwen server, e.g. `"French"`, `"Japanese"`, `"German"`; `null` falls back to `"English"` | `null` | Qwen pronunciation language; not text translation. |
| `settings.qwen_voice_clone_audio_path` | string or null | Readable reference-audio path for the configured cloning service, or `null` | `null` | A non-empty path activates cloning even if `qwen_voice_clone_enabled` is false. Only use voices you have permission to use. |
| `settings.qwen_voice_clone_enabled` | boolean | `true`, `false` | `false` | True requires a non-empty reference-audio path. False does not disable cloning while a path remains set. |
| `settings.qwen_voice_clone_prompt` | string or null | Reference transcript/prompt or `null` | `null` | Optional reference prompt passed to Qwen voice cloning. |
| `settings.minimax_voice_id` | string or null | MiniMax voice ID available to your account, or `null` | `null` | Used only for MiniMax TTS, which also requires its API key. |
| `settings.minimax_tts_model` | string or null | MiniMax speech model ID, or `null` → `"speech-02-hd"` | `null` | MiniMax TTS model override; availability is remote/provider-specific. |
| `settings.mimo_voice_id` | string or null | MiMo voice ID, or `null` → `"mimo_default"` | `null` | Voice used with `voice_tts_type="mimo_tts"`. |
| `settings.mimo_tts_type` | string or null | Intended `"builtin"`, `"voicedesign"`, `"voiceclone"`; null/other strings select builtin model | `null` | Selects `mimo-v2.5-tts`, `mimo-v2.5-tts-voicedesign`, or `mimo-v2.5-tts-voiceclone`. Gateway still calls the builtin-style synthesis method, so design/clone modes are not fully wired end to end. |
| `settings.mimo_tts_base_url` | string or null | MiMo-compatible TTS API base URL including the API root (e.g. `"https://api.xiaomimimo.com/v1"`), or `null` | `null` | Overrides the MiMo TTS endpoint for `voice_tts_type="mimo_tts"`. Resolution order: this variable → environment `MIMO_TTS_API_BASE` → built-in default `https://api.xiaomimimo.com/v1`. |
| `settings.mimo_tts_model` | string or null | Any model ID served by the configured endpoint, or `null` | `null` | Free-form MiMo TTS model override. When `null`, the model follows `mimo_tts_type` (`mimo-v2.5-tts`, `mimo-v2.5-tts-voicedesign`, `mimo-v2.5-tts-voiceclone`). |

## LLM, history, and compaction

| Variable | Type | Allowed values / operational constraints | Default | Effect |
|---|---|---|---|---|
| `settings.provider` | string or null | `null` = `USE_PROVIDER`; registered names `"openai"`, `"anthropic"`, `"ollama"`, `"llamacpp"`, `"minimax"`, `"mimo"`, `"openrouter"` | `null` | Context chat-provider override. Provider must be configured; saving an arbitrary name does not register it. |
| `settings.model` | string or null | Exact model ID/tag supported by the selected provider, or `null` for its configured default | `null` | Chat model override. Ollama Cloud tags must match your available tag exactly. Not an STT/TTS model selection. |
| `settings.thinking_mode` | string | `"auto"`, `"on"`, `"off"`, `"low"`, `"medium"`, `"high"`, `"xhigh"`; provider support varies | `"auto"` | Provider-specific reasoning generation hint on both message paths; distinct from output visibility and task limits. |
| `settings.show_thinking` | boolean | `true`, `false` | `false` | User-only visibility of provider-supplied reasoning in a separate display. Never substitutes reasoning for a final answer or feeds it to TTS. |
| `settings.vision_provider` | string or null | Same registered provider names as above, with image-capable provider/model support; `null` falls back to `VISION_PROVIDER` | `null` | Vision-provider hint when requests contain images. Actual support/routing depends on provider and streaming path. |
| `settings.vision_model` | string or null | Image-capable model ID or `null` → `VISION_MODEL`/provider default | `null` | Vision model override; does not add vision support to a text-only model. |
| `settings.max_llm_turns` | I32 or null | `null` or ≤1 selects the chat path; **2–I32_MAX** selects multi-turn; use small positive limits | `null` → `1` | Chat supports successive tool-only responses until its tool budget is consumed, then at most one finalization request without tools. Multi-turn counts LLM rounds; exhausting it without a final answer reports a limit, never an older answer. Neither setting is a strict billing cap because provider retries are separate. |
| `settings.max_tool_calls` | I32 or null | `null` → `5`; use ≥0; ≤0 disallows tool execution | `null` | Chat: total attempted calls per incoming message, snapshotted at task start; invalid/unknown calls consume budget too. Multi-turn: execution limit per LLM round. Skipped calls receive saved error results. Larger tasks need explicit multi-turn configuration and suitable limits. |
| `settings.history_with_toolcalls` | boolean | `true`, `false` | `true` | In agent-loop history, false excludes older tool conversations, but retains the current task's tool calls/results so skill loading and other chains can continue. Chat includes tool history. See partial-JSON exception. |
| `settings.only_tool_calls_no_history` | boolean | `true`, `false` | `false` | **Stored only:** does not currently filter history. |
| `settings.summarize_char_limit` | I32 or null | `null` or representable integer; positive count recommended | `null` | **Stored only:** not the compaction threshold. Use token settings below. |
| `settings.history_token_limit` | Usize or null | `null` → `32000`; 0 allowed, positive practical budget recommended | `null` | Estimated history-token budget, not the model tokenizer. **The newest complete user turn, including its tool chain, stays intact even when oversized**, including at 0. Provider preflight must budget the full request against the actual model window. |
| `settings.tool_result_limit` | Usize or null | Retired, read-only legacy compatibility | `null` | Accepted when loading old contexts but no longer serialized as an active setting. Use per-call `_output` selections or `read_tool_result`; complete snapshots remain stored. |
| `settings.compaction_enabled` | boolean | `true`, `false` | `false` | Enables automatic pre-request compaction in chat and agent flows. Manual `/compact` also sets it true. A previously saved summary remains visible even when automatic compaction is off. |
| `settings.compaction_summary` | string | Any text; `""` = no summary | `""` | Persisted conversation summary injected in chat and agent system prompts, including minimal/custom templates. Not appended twice when already rendered by the template. May contain private conversation data. |
| `settings.compaction_token_limit` | Usize or null | `null` → `8000`, capped at half the history budget, minimum 1 | `null` | Pre-request compaction threshold in chat and agent flows. Only older complete turns are eligible; the current request/tool chain remains. Failure leaves history unchanged. Not a model tokenizer/window limit. |
| `settings.compaction_template` | string or null | Existing template name without `.poml`, e.g. `"compaction"`; `null` uses `"compaction"` | `null` | Compaction prompt selection. A missing template uses the built-in fallback. Compaction itself makes an LLM request. |
| `settings.agent_name` | string | Display/log label; any string | `"assistant"` | Currently used in agent logging, not the Discord bot name, wake word, or model persona. See partial-JSON exception. |

## Workflow, templates, tools, and feedback

| Variable | Type | Allowed values / operational constraints | Default | Effect |
|---|---|---|---|---|
| `settings.system_template` | string or null | Existing name relative to `templates/`, **without `.poml`**; e.g. `"standard"`, `"language_instructor"`, `"roles/researcher"`; null → `"standard"` | `null` | System POML selected by both message paths after routing. Invalid/missing explicit selections error instead of falling back. Empty string, traversal and absolute paths are invalid. |
| `settings.sm_file` | string or null | Relative workflow name under `contexts/`, or `null`; `.sm`/`.cl` extension can be omitted | `null` | Workflow override on both message paths; takes precedence over root `sm_file`. `.sm` is preferred to legacy `.cl`. A malformed `.sm` is not hidden by a fallback. Legacy `settings.cl_file` updates are normalized before merging. |
| `settings.active_skill` | string or null | Registered skill name, or `null` for no active skill | `null` | `/skill skillname:NAME` selects persistent instructions for subsequent tasks; `off` clears. The current raw task supplies required code/error/user_request arguments. Does not execute scripts or enable disabled tools. Persistent selection is human-only; agent context tools and SM transitions cannot change it. |
| `settings.active_state` | string or null | Workflow state name or `null` | `null` | Compatibility state marker synchronized with root `active_state` when shared workflow routing runs. |
| `settings.active_templates` | array of strings | Template names without `.poml`, or `[]` | `[]` | Workflow stack modified by push/pop/next actions and tags. Does not directly select the system template. |
| `settings.current_template` | string | Member of the settings template stack, or `""` | `""` | Cursor used by `agent_next`; not the system POML selection. |
| `settings.done` | boolean | `true`, `false` | `false` | Current-task completion set by `agent_complete`; the agent loop checks it after tools. Automatically reset before a NEW task, not during tool continuations/retries/previews. |
| `settings.path` | string | Directory/path text or `""` | `""` | Workflow metadata updated by path actions/tags. Does **not** change process cwd; ordinary file/terminal operations do not reliably honor it. Use explicit paths. |
| `settings.tags_enabled` | boolean | `true`, `false` | `false` | Enables configurable-prefix tag parsing/instructions in the agent path. Tags can cause workflow actions. Separate `[[AGENT:...]]` signals also exist. |
| `settings.tag_prefix` | string | 1–16 characters; no whitespace/control characters | `"§"` | Literal command prefix; e.g. `!praxis/done`. Historical `§done` syntax still works with the default. User or trusted workflow-author configuration, not model-writable. Exposed to POML as `settings.tag_prefix`. |
| `settings.decision_profile` | string or null | Existing profile name in `decisions/*.json`; `"off"` or null disables | `null` | User/workflow-author policy to enable the Decision router. Model/tool updates are rejected. Selecting a profile does not grant additional tool permissions.
| `settings.use_freerouter` | boolean | `true`, `false` | `false` | When `true`, bypass the GPU router (pgpu slot wake/wait logic) and use the pgpu free router endpoint directly (`/free/v1` on the dashboard port). The free router must be configured in pgpu with at least one enabled provider. Useful for GPU-free operation or when GPU quota is exhausted. Does not affect Decision endpoint usage. |
| `settings.use_decision_router` | boolean | `true`, `false` | `true` | Explicit toggle for the Decision Router. When `false`, automatic state routing is skipped regardless of `decision_profile`. Useful to disable routing for specific states or testing. Does not affect `decision_profile` storage. |
| `settings.activated_tools` | array of strings | Tool names (built-in or plugin) and/or group names from `settings.tool_group_definitions`, e.g. `["coding", "execute_terminal", "brave_web_search"]`. | `[]` | The state's tool allow-list. When non-empty (or when a category is listed), only these tools are sent to the model, all with full schemas; unlisted built-in **and plugin** tools are withheld. Set per state in the `.sm`. |
| `settings.tool_group_definitions` | object: name → array of strings | Group name (`[A-Za-z0-9_-]`) → members: tool names or other group names (cycles tolerated). | `{}` | There are **no built-in groups**. A workflow `[tool_groups]` section is merged in on every routing pass (same-named entries replaced so `.sm` edits propagate; other user entries persist). Users add groups via `set_context settings.tool_group_definitions.<name> = [...]`. |
| `settings.full_tool_schemas` | array of strings | Tool names. | `[]` | Legacy alias; merged into `activated_tools`. |
| `settings.tool_groups` | array of strings or null | Group names. | `null` | Legacy alias; merged into `activated_tools`. |
| `settings.full_tool_categories` | array of strings | `Action`, `Discovery`, `SkillLoader`, `AgentControl`, `Memory`, `Context`. | `[]` | Additionally allows every built-in tool of the listed categories (union with `activated_tools`). |
| `settings.tool_discovery_mode` | string | `Full`, `DescriptionOnly`, `None` | `DescriptionOnly` | `Full` = full schemas for allowed tools; `DescriptionOnly` = full schemas for activated/discovered tools, descriptions for other allowed tools; `None` = only activated/discovered tools, with full schemas. Discovery is task-local and cannot enable disabled tools. Explicit allow-lists resolving to empty/cyclic groups stay closed. |
| `settings.llm_turn` | I32 | Representable integer; ≥0 recommended | `0` | Incremented by `agent_next`; not the same as root `turn`, total provider requests, or token usage. |
| `settings.tool_history_limit` | Usize | **0** = retain no entries; **1–USIZE_MAX** retains that many recent entries; use a modest number | `50` | Agent-loop `custom_data.tool_history` retention; the last-call summary still exists at 0. Exposed to POML as `used_tools_history_size`. Not an execution limit. |
| `settings.download` | boolean | `true`, `false`; user-only | `false` | Opt-in permission to save Discord attachments. Disabled/unreadable policy performs no attachment download. Guild/channel permission checks run first. This is not a general network sandbox. |
| `settings.feedback_enabled` | boolean | `true`, `false`; user-only | `false` | Enables external agent feedback (Discord/TTS). Required even with non-empty `feedback_mode`. Feedback actions cannot enable it. Web progress and final replies are unaffected. |
| `settings.feedback_max_per_5min` | I32 | 0–1000 | `10` | Maximum external feedback messages per user/session in the configured sliding window (legacy name); 0 disables delivery. Shared across tasks. |
| `settings.feedback_window_secs` | I32 | 1–86400 seconds | `300` | Sliding feedback-rate window. Invalid saved limits fail closed. |
| `settings.feedback_mode` | array of strings | Any subset of `["tts", "dm", "text"]`; `[]` allowed; unknown entries have no defined routing | `[]` | Agent feedback destinations. `text` uses a valid channel or falls back to DM; `tts` uses the voice backend. Empty/unhandled modes can still trigger fallback TTS for voice input/use_tts. |
| `settings.feedback_channel_id` | string or null | Discord text-channel numeric ID **as a string**, or `null` | `null` | Feedback channel override and fallback for tool `channel_id`. `voice:<guild>` is an internal input marker, not a valid text channel. |
| `settings.upload_channel_id` | string or null | Discord channel numeric ID as a string, or null | `null` | Agent-loop upload fallback when no tool argument/originating channel is available; does not override an explicit tool channel. |
| `settings.feedback_template` | — | Removed | — | Old saved values are ignored and no longer written back. Feedback is delivered directly; `tasks/feedback` remains an explicitly selectable task template. |
| `settings.message_on_toolcalling` | boolean | `true`, `false` | `false` | Enables pre-tool feedback when `feedback_enabled=true`; external delivery uses the same sliding-window limiter. Not ordinary final reply TTS. |
| `settings.allowed_guilds` | array of strings | `["*"]` = all; guild numeric IDs as strings; `[]` = no guild matches | `["*"]` | Allow-list checked for ordinary paired Discord text messages. DMs bypass the guild check. Not a global/voice ACL; voice authorization is pairing-based. |
| `settings.allowed_channels` | array of strings | `["*"]` = all; channel numeric IDs as strings; `[]` = none | `["*"]` | Allow-list checked for ordinary Discord text messages, not an enforced voice-channel allow-list. |

## VM-related context settings

These do not enable or start a VM. VM startup, RAM, architecture, and disk settings are separate environment/service settings.

| Variable | Type | Allowed values / operational constraints | Default | Effect |
|---|---|---|---|---|
| `settings.vm_screenshot_enabled` | boolean | `true`, `false` | `true` | Requests automatic screenshots after relevant VM actions in the agent loop. Does not prevent explicit screenshot tools. |
| `settings.vm_screenshot_limit` | Usize | 0–USIZE_MAX representable; small practical count recommended | `5000` | **Stored only:** currently not an enforced screenshot-retention limit. |
| `settings.vm_keyboard_layout` | string | Layouts `"us"`, `"de"`, `"fr"`, `"es"`, `"it"`, `"gb"`; case-insensitive. Aliases: `german`, `qwertz`, `french`, `azerty`, `spanish`, `italian`, `uk`, `british`; unknown values select US | `"us"` | Keyboard mapping used by VM typing helpers. Requires a matching guest layout. Existing mapping limitations are not a TTS/STT setting. |

## Runtime POML variables — not additional saved settings

POML uses JavaScript expressions. A missing identifier can throw even in `if="user_prompt"`. Guard optional variables with `typeof`, for example:

```xml
<cp caption="Request" if="typeof user_prompt === 'undefined' ? false : user_prompt">
  <p>{{ user_prompt }}</p>
</cp>
```

The shipped templates guard optional variables. Both runtime paths and dashboard preview now use `src/gateway/prompt.rs` for a shared context contract. Routing sees the current raw input first. Preview does not persist its routing changes and never substitutes the latest assistant/tool output for user input. The latest user input is also a normal conversation message.

The complete synthetic render fixture is [`examples/poml-test-context.json`](../examples/poml-test-context.json). Run `python3 scripts/test_poml_templates.py` with `POML_CLI` set to test every template against full, empty, null and chat contexts. This reference describes source behavior; an older deployed binary requires rebuilding/updating first.

| Runtime variable | Type / allowed content | Availability and meaning |
|---|---|---|
| `user_id` | Internal ID string | Shared user/system/preview context, scoped to the current Praxis user. |
| `session_id` | Session ID string | Current user session. |
| `settings` | Canonical settings object | Typed configuration; no secrets registry is included. Effective `settings.system_template` defaults to `standard`. |
| `sm_file` | Workflow-name string | Effective selection (`settings.sm_file`, then root, then `standard`). |
| `system_template` | Template-name string | Effective alias of `settings.system_template`, default `standard`. |
| `active_templates` | Array of strings | Current root workflow template metadata; not a replacement for system-template selection. |
| `username` | Display string | Both system paths; null saved username normally becomes `"User"`. |
| `turn` | Integer | Both system paths; root message counter. |
| `mode` | String, normally `agent`/`chat` | Both system paths; root mode. |
| `user_prompt` | Current message string | Current raw input on both runtime paths and in preview. Also refreshed in `custom_data.user_prompt` before SM routing. Not an `.env` variable. |
| `user_message` | Current message string | Alias of `user_prompt` in the shared builder. |
| `system_info` | String | Both system paths; `Praxis v<version>`. |
| `time` | Local datetime string | Both system paths; `YYYY-MM-DD HH:MM:SS`. |
| `utc_now` | ISO UTC datetime string | Shared builder timestamp for deterministic SRS due-date comparisons and scheduling; not a saved setting. |
| `path` | Path string | Non-empty `settings.path`, otherwise process cwd, consistently across the shared builder. Metadata does not change cwd. |
| `skills` | Array of bounded metadata summaries | Candidates selected by the POML discovery plan, at most 20; default `[]`, **not the complete catalog**. Contains name, description, required_parameters, skill_hidden and user_only. Instructions load only on use. See [Skills](SKILLS.md). |
| `skill_discovery_instructions` | String | Discovery guidance rendered from `templates/discovery/skills.poml` (or the context-selected variant). Shown by the shared runtime include in agent mode. |
| `tools` | Array of `{name, description, parameters}` | Enabled core tools plus at most 24 task-local discoveries from search_tools, shared by provider schemas/runtime/preview. Not the complete catalog. See [Tool discovery and memory](TOOL_DISCOVERY.md). Discovery grants no permission to act. |
| `active_skill` | Skill-name string | Selected skill, or `""` when off. |
| `active_skill_instructions` | String | Strictly rendered instructions using the current task, or `""`. Runtime also appends these separately so a custom system template cannot accidentally omit them. Loading does not mean execution. |
| `skills[].name` | Skill-name string | Identifier loaded from a valid local skill definition. |
| `skills[].description` | Description string | Human-readable description from the skill definition. |
| `skills[].required_parameters` | Array of parameter-name strings | Required non-empty string inputs to pass inside `use_skill.parameters`; `[]` for older manifests without this field. |
| `tools[].name` | Tool-name string | Identifier from an enabled tool definition; not arbitrary user input. |
| `tools[].description` | Description string | Tool description supplied to the renderer. |
| `tools[].parameters` | JSON Schema object | Argument schema, including provider/tool-defined nested properties and required fields. |
| `memory` | Object | Both system paths; contains the four fields listed below. |
| `memory.facts` | Array of strings | Current user's learned facts. Empty when none; storage errors propagate rather than masquerading as empty memory. |
| `memory.topics` | Array of strings | Current user's tracked topics; no other user's entries. |
| `memory.preferences` | Object | Preference-name → arbitrary JSON value mappings. Empty object when none. |
| `memory.variables` | Object | Arbitrary named JSON memory values. Empty object when none. |
| `custom_data` | JSON, operationally object | Saved custom data, normalized from null to `{}`. Not flattened into top-level variables. |
| `sm_data` | JSON, operationally object | Saved statemachine data, normalized from null to `{}`. Old POML can still read a renderer-only `cl_data` compatibility alias; public/saved context JSON uses `sm_data`. |
| `used_tools_history_size` | Non-negative integer | Both system paths; alias for `settings.tool_history_limit` (0 = no retained entries). |
| `active_state` | State-name string | Routed root state (settings fallback); `""` when unset. |
| `tag_instructions` | Instruction string | Tag syntax when `settings.tags_enabled` is true; otherwise `""`. |
| `uptime` | Human-readable duration string | Shared runtime metadata. Preview uses its supplied uptime. |
| `uptime_secs` | Non-negative integer | Supplied process uptime in seconds. |
| `paired_users_count` | Non-negative integer | Number of pairing records **for this user**, not all users. |
| `paired_users` | Array of pairing objects | Pairings scoped to this user: `user_id`, `discord_user_id`, `paired_at`. Avoid repeating identifiers unnecessarily. |
| `paired_users[].user_id` | Internal ID string | Pairing's Praxis identity. |
| `paired_users[].discord_user_id` | Numeric ID string | Pairing's Discord identity. |
| `paired_users[].paired_at` | Timestamp string | Pairing creation time. |

The shared builder also supplies these metadata variables. Dedicated compaction calls still need only `conversation_text`:

| Runtime variable | Type / allowed values | Availability and meaning |
|---|---|---|
| `user_template` | Template-name string, default `"user"` | Alias of `custom_data.user_template`; optional current-message wrapper. |
| `conversation_text` | String | This user's history as `role: content` lines. Potentially private; do not dump unnecessarily. |
| `tokens_used` | Non-negative integer | Estimated history tokens; can exceed the configured limit. |
| `tokens_limit` | Non-negative integer, default `500000` | Effective history budget. |
| `tokens_percentage` | Decimal **string**, `"0.0"`–`"100.0"` | Capped history usage percentage; zero when budget is zero. |
| `compaction_token_limit` | Positive integer, default `8000` | Effective shared pre-request compaction threshold (capped at half the history budget, minimum 1). |
| `compaction_percentage` | Decimal **string**, `"0.0"`–`"100.0"` | Capped threshold usage percentage; zero when threshold is zero. |
| `message_count` | Non-negative integer | Number of loaded history messages. |

`tokens_remaining`, `vm_enabled`, `vm_name`, `running_vms`, and `installation_disks` are **not supplied by these current system/preview builders**. Old templates may reference them, but setting unrelated environment variables does not make those POML identifiers exist. The user-message render now receives the same shared variables; a dedicated compaction render still receives only `conversation_text`.

## Built-in custom-data and workflow namespaces

There is no finite list of user/plugin-defined keys: `custom_data` and `sm_data` are explicitly open JSON namespaces. Their allowed values are JSON strings, numbers, booleans, arrays, objects, and null. The following keys have built-in uses; new application-specific keys must be documented by their owner.

| Variable | Type / allowed values | Default/source and meaning |
|---|---|---|
| `custom_data.user_template` | Template-name string without `.poml` | Runtime default `"user"`; optional user-message POML in the agent loop. |
| `custom_data.channel_id` | Discord channel ID string, or internal frontend marker | Default Discord tool destination, inserted from incoming/fallback channel when absent. Can be stale across frontends; not a voice-channel selection. |
| `custom_data.user_prompt` | String | Refreshed with the current raw input **before** workflow routing and rendering on both paths; POML `user_prompt` and `user_message` receive the same current input. |
| `custom_data.tool_history` | Array of tool-entry objects | Agent-loop-generated history. Entries contain `tool`, `args`, `result`, `timestamp`, `iteration`, `duration_ms`; limited by `tool_history_limit`. |
| `custom_data.tool_history[].tool` | Tool-name string | Executed tool name. |
| `custom_data.tool_history[].args` | JSON | Tool arguments; may contain private data. |
| `custom_data.tool_history[].result` | String | Tool result text. |
| `custom_data.tool_history[].timestamp` | Timestamp string | Tool execution timestamp. |
| `custom_data.tool_history[].iteration` | Non-negative integer | Agent iteration/turn associated with the call. |
| `custom_data.tool_history[].duration_ms` | Non-negative integer | Execution duration in milliseconds. |
| `custom_data.used_tools` | Object | Agent-loop-generated last-call summary, not a setting that enables tools. |
| `custom_data.used_tools.last_call` | Tool-name string | Most recent tool name. |
| `custom_data.used_tools.last_args` | JSON | Most recent tool arguments. |
| `custom_data.used_tools.last_result` | String | Most recent result text. |
| `custom_data.used_tools.count` | Non-negative integer | Current agent-loop iteration at the last call, **not** a total tool-call count; several calls can share the same count. |
| `custom_data.used_tools.history` | Array of tool-entry objects | Copy of the retained `custom_data.tool_history`; same six entry fields and types. Empty when retention is 0. |
| `custom_data.vm_last_screenshot` | Path string | Last automatic VM screenshot, consumed by the agent's image/history path. |
| `custom_data.mode` | Any JSON; legacy tools usually write a string | Separate custom value used by some legacy actions. Does not change `Context.mode`. |
| `custom_data.pref_<name>` | JSON | Legacy preference copies. New preference tools write typed durable `memory.preferences`; do not create new `pref_` copies. |
| `custom_data.<plugin-key>` | Plugin-defined JSON | Enabled plugins can supply defaults; existing custom values take precedence. See the plugin manifest/handler for each plugin's allowed values. |
| `custom_data.comfyui_*` | Legacy ComfyUI strings, or integer timeout, or null | Backward-compatible fallback for the matching `settings.comfyui_*` fields above. Typed settings take precedence; use settings for new configuration. See [ComfyUI TTS](COMFYUI.md). |
| `custom_data.language_learning` | Object | Tutor overrides: `target_language`, `explanation_language`, `level`, `lesson_goal`, `reply_style`. See [POML workflows](POML_WORKFLOWS.md). |
| `custom_data.semantic_blueprint` | Object | Semantic goal, participants, action, constraints, success criteria and output-format overrides. Not a tool permission grant. |
| `sm_data.semantic_blueprint` | Object | Workflow-provided blueprint override; lower priority than `custom_data.semantic_blueprint`. |
| `sm_data.<name>` | Workflow-defined JSON | Free-form workflow data, preferably objects for dotted updates. The selected workflow defines allowed keys and values. |
| `custom_data.skill_discovery_template` | Template-name string | Optional POML discovery plan, without `.poml`; default `discovery/skills`. Configure instructions, pinned names, queries and result count here; access flags and safety bounds remain enforced. |

SM files also have their own transient variables, conditions, state names, and overrides. Those are workflow definitions rather than additional fields of `ContextSettings`. Unknown typed root/settings fields do not survive a normal context save; use the documented open namespaces for new persistent data.

## Documentation coverage check

From the source checkout:

```bash
python3 scripts/check_context_docs.py
```

The check compares the reference table against both Rust context structs, rejects missing/duplicate/unknown fields, and requires type, allowed-values, default, and effect cells for every one. Code/runtime behavior remains authoritative when a provider or feature changes.
