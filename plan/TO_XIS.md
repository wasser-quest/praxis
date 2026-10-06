# Long horizon: everything before `xis`

One ordered task list from today's pluginized kernel to the `xis` package
manager ([design](../docs/XIS_PACKAGE_MANAGER.md), handoff §6I). Tasks are
finished only when their verification runs green (handoff §8).

## Task 1 — voice/audio as a provider crate

`crates/praxis-voice`: transcription, synthesis and audio utilities with plain
settings (no host types), owning the ComfyUI/XTTS/Qwen3 backends. The kernel
keeps a thin mapping from typed context settings. The Discord voice glue moves
under `src/discord` (it is channel code, gated with `discord`).

Verify: `cargo test -p praxis-voice`, kernel `--lib` both configurations.

## Task 2 — channel ingress seam and `praxis-channel-discord`

The Discord bot is already a gateway WS/REST client; what it takes from the
kernel becomes an explicit `ChannelHost` seam (~12 methods: agent control,
interactive input, delivery events, context commands, skill lookup and the
tool/credential/message state it reads). `src/discord` then compiles as
`crates/praxis-channel-discord` behind the `discord` feature, with its
`packages/discord_service` manifest and scoped `discord_bot_token` credentials.

Verify: crate tests + kernel `--lib` both configurations + a core-only build
that carries neither `serenity` nor `songbird`.

## Task 3 — §6F minimal kernel

A `minimal` build/run profile: `file_ops` + plugin management only. No provider
router, cron scheduler, media service, retention, VM, dashboard, voice or
channel worker is initialized. The remaining heavy deps (`image`, audio, TLS)
leave the dependency set with their last consumers.

Verify: `cargo tree --no-default-features -p praxis` shows no `serenity`,
`songbird`, `whisper-rs`, `vosk`, `image`, audio or TLS crates; the minimal
binary starts, manages plugins and runs `file_ops` tools only.

## Task 4 — §6D acceptance run

The acceptance of handoff §6D, automated: provider-only chat, the verified
Rust workflow and language teaching all work on a build with **no** VM, UI or
media.

Verify: one scripted run over the three flows on a core-only host.

## Task 5 — `xis` (handoff §6I)

Arrive here with the kernel and packages in the shape the design assumes.
Phases from the design: (1) read-only repositories (`repo add/list/refresh`,
`search`, `plan`), (2) setup install (`keep/backup/overwrite`,
`xis.lock.json`, plugin delegation to the Praxis CLI), (3) MOTD service,
(4) index signatures, pinned repo keys and immutable-version enforcement.
`xis` is a separate executable that links nothing of the kernel.
