use std::io::{self, Write};

pub async fn run_interactive_onboard() -> anyhow::Result<()> {
    println!();
    println!("========================================");
    println!("   Praxis AI Agent Platform - Setup");
    println!("========================================");
    println!();

    // Load existing .env if present
    let existing = load_existing_env();
    let has_existing = !existing.is_empty();
    if has_existing {
        println!("Found existing .env configuration.");
        println!("Press Enter to keep current values (shown in brackets).");
        println!();
    }

    let mut env_lines: Vec<String> = Vec::new();

    // POML CLI
    println!("--- POML CLI ---");
    let poml_cli = prompt_with_default(
        "POML CLI Path",
        &get_existing(&existing, "POML_CLI", "./poml/js/cli.cjs"),
    );
    env_lines.push(format!("POML_CLI={}", poml_cli));
    println!();

    // LLM Provider
    println!("--- LLM Provider ---");
    let existing_provider = get_existing(&existing, "USE_PROVIDER", "openai");
    let has_provider = existing.contains_key("USE_PROVIDER");

    let provider_name = if has_provider {
        println!("Current provider: {}", existing_provider);
        println!("  1) Keep current provider and keys (skip)");
        println!("  2) OpenAI (GPT-4)");
        println!("  3) Anthropic (Claude)");
        println!("  4) Llama.cpp (Local Server)");
        println!("  5) Ollama (Local)");
        println!("  6) MiniMax");
        println!("  7) MiMo");
        println!();

        let skip_choice = prompt_choice("Select provider", &["1", "2", "3", "4", "5", "6", "7"], "1")?;
        if skip_choice == "1" {
            // Keep all existing provider config
            env_lines.push(format!("USE_PROVIDER={}", existing_provider));
            for (key, val) in &existing {
                if key.starts_with("OPENAI_")
                    || key.starts_with("ANTHROPIC_")
                    || key.starts_with("LLAMACPP_")
                    || key.starts_with("OLLAMA_")
                    || key.starts_with("MINIMAX_")
                    || key.starts_with("MIMO_")
                    || key.starts_with("OPENROUTER_")
                {
                    if !env_lines
                        .iter()
                        .any(|l| l.starts_with(&format!("{}=", key)))
                    {
                        env_lines.push(format!("{}={}", key, val));
                    }
                }
            }
            println!("Keeping current provider config.\n");
            existing_provider.clone()
        } else {
            let real_choice = match skip_choice.as_str() {
                "2" => "1",
                "3" => "2",
                "4" => "3",
                "5" => "4",
                "6" => "5",
                "7" => "6",
                _ => "1",
            };
            select_provider(real_choice, &existing, &mut env_lines)?
        }
    } else {
        println!("Which LLM provider do you want to use?");
        println!("  1) OpenAI (GPT-4)");
        println!("  2) Anthropic (Claude)");
        println!("  3) Llama.cpp (Local Server)");
        println!("  4) Ollama (Local)");
        println!("  5) MiniMax");
        println!("  6) MiMo");
        println!();

        let provider_choice = prompt_choice("Select provider", &["1", "2", "3", "4", "5", "6"], "1")?;
        select_provider(&provider_choice, &existing, &mut env_lines)?
    };
    persist_provider_selection(&mut env_lines, &provider_name);
    for (key, value) in &existing {
        if key.starts_with("LLM_") { env_lines.push(format!("{key}={value}")); }
    }
    println!();

    // Embedding Model (for RAG)
    println!("--- Embedding Model (for RAG) ---");
    println!("Embeddings are used for semantic search in the knowledge base.");
    println!("  1) Ollama (local, free) - Recommended");
    println!("  2) OpenAI (cloud, paid) - Best quality");
    println!("  3) None (hash-based, lowest quality)");
    let has_embedding = existing.contains_key("EMBEDDING_PROVIDER");
    let embedding_default = if has_embedding {
        match get_existing(&existing, "EMBEDDING_PROVIDER", "ollama").as_str() {
            "openai" => "2",
            "ollama" => "1",
            _ => "3",
        }
    } else {
        "1"
    };
    let embedding_choice = prompt_choice("Select embedding provider", &["1", "2", "3"], embedding_default)?;
    match embedding_choice.as_str() {
        "1" => {
            let base_url = prompt_with_default(
                "Ollama API Base URL",
                &get_existing(&existing, "OLLAMA_API_BASE", "http://localhost:11434"),
            );
            let model = prompt_with_default(
                "Embedding Model",
                &get_existing(&existing, "EMBEDDING_MODEL", "nomic-embed-text"),
            );
            env_lines.push("EMBEDDING_PROVIDER=ollama".to_string());
            env_lines.push(format!("EMBEDDING_MODEL={}", model));
            // Only add OLLAMA_API_BASE if not already set
            if !env_lines.iter().any(|l| l.starts_with("OLLAMA_API_BASE=")) {
                env_lines.push(format!("OLLAMA_API_BASE={}", base_url));
            }
            println!("Embeddings: Ollama {} (local)", model);
        }
        "2" => {
            let api_key = prompt_required_with_existing(
                "OpenAI API Key",
                &get_existing(&existing, "OPENAI_API_KEY", ""),
            );
            let base_url = prompt_with_default(
                "OpenAI API Base URL",
                &get_existing(&existing, "OPENAI_API_BASE", "https://api.openai.com/v1"),
            );
            let model = prompt_with_default(
                "Embedding Model",
                &get_existing(&existing, "EMBEDDING_MODEL", "text-embedding-3-small"),
            );
            env_lines.push("EMBEDDING_PROVIDER=openai".to_string());
            env_lines.push(format!("EMBEDDING_MODEL={}", model));
            // Only add OPENAI_API_BASE if not already set
            if !env_lines.iter().any(|l| l.starts_with("OPENAI_API_BASE=")) {
                env_lines.push(format!("OPENAI_API_BASE={}", base_url));
            }
            // Store API key for secrets
            if !env_lines.iter().any(|l| l.starts_with("OPENAI_API_KEY=")) {
                env_lines.push(format!("OPENAI_API_KEY={}", api_key));
            }
            println!("Embeddings: OpenAI {} (cloud)", model);
        }
        "3" => {
            println!("Embeddings: Hash-based (no configuration needed)");
        }
        _ => {}
    }
    println!();

    // Discord (optional)
    println!("--- Discord Bot (Optional) ---");
    let has_discord = existing.contains_key("DISCORD_BOT_TOKEN");
    let discord_default = if has_discord { true } else { false };
    let discord_label = if has_discord {
        "Update Discord config?"
    } else {
        "Setup Discord?"
    };
    let setup_discord = prompt_yes_no(discord_label, discord_default)?;

    if setup_discord {
        let bot_token = prompt_required_with_existing(
            "Discord Bot Token",
            &get_existing(&existing, "DISCORD_BOT_TOKEN", ""),
        );
        let app_id = prompt_required_with_existing(
            "Discord Application ID",
            &get_existing(&existing, "DISCORD_APPLICATION_ID", ""),
        );
        env_lines.push(format!("DISCORD_BOT_TOKEN={}", bot_token));
        env_lines.push(format!("DISCORD_APPLICATION_ID={}", app_id));
    } else if has_discord {
        // Keep existing values
        env_lines.push(format!(
            "DISCORD_BOT_TOKEN={}",
            existing["DISCORD_BOT_TOKEN"]
        ));
        if let Some(app_id) = existing.get("DISCORD_APPLICATION_ID") {
            env_lines.push(format!("DISCORD_APPLICATION_ID={}", app_id));
        }
    }
    println!();

    // Gateway
    println!("--- Gateway ---");
    let gateway_port = prompt_with_default(
        "Gateway Port",
        &get_existing(&existing, "GATEWAY_PORT", "3537"),
    );
    let gateway_api_key = prompt_with_default_or_generate(
        "Gateway API Key",
        32,
        &get_existing(&existing, "GATEWAY_API_KEY", ""),
    );
    env_lines.push(format!("GATEWAY_PORT={}", gateway_port));
    env_lines.push(format!("GATEWAY_API_KEY={}", gateway_api_key));
    println!();

    // Dashboard
    println!("--- Dashboard ---");
    let dashboard_port = prompt_with_default(
        "Dashboard Port",
        &get_existing(&existing, "DASHBOARD_PORT", "1337"),
    );
    let admin_password = prompt_password_with_existing(
        "Dashboard Admin Password",
        &get_existing(&existing, "DASHBOARD_ADMIN_PASSWORD", ""),
    )?;
    env_lines.push(format!("DASHBOARD_PORT={}", dashboard_port));
    env_lines.push(format!("DASHBOARD_ADMIN_PASSWORD={}", admin_password));
    println!();

    // Master password for encrypted secrets
    println!("--- Encryption ---");
    let has_enc2 = std::path::Path::new("secrets.enc2").exists();
    let master_password = if has_enc2 {
        println!("Encrypted secrets file exists.");
        let re_encrypt = prompt_yes_no("Re-encrypt secrets with new password?", false)?;
        if re_encrypt {
            prompt_password_with_existing("Set MASTER_KEY password", "")?
        } else {
            let pw = rpassword::prompt_password(
                "Enter existing MASTER_KEY (required to save secrets): ",
            )?;
            pw
        }
    } else {
        println!("All secrets will be encrypted with a master password (enc2).");
        prompt_password_with_existing("Set MASTER_KEY password", "")?
    };
    println!();

    // VM (optional)
    println!("--- Virtual Machine (Optional) ---");
    println!("Enable QEMU VM support? The LLM can control a full Linux VM.");
    println!("Requires: qemu-system-x86_64 (apt install qemu-system-x86)");
    let has_vm = existing.contains_key("VM_ENABLED");
    let vm_default = existing
        .get("VM_ENABLED")
        .map(|v| v == "true")
        .unwrap_or(false);
    let vm_label = if has_vm {
        "Update VM config?"
    } else {
        "Enable VM mode?"
    };
    let setup_vm = prompt_yes_no(vm_label, vm_default)?;

    if setup_vm {
        env_lines.push("VM_ENABLED=true".to_string());

        // CPU Cores
        let cpu_cores = prompt_with_default(
            "VM CPU Cores (1-8, more = faster but uses more host resources)",
            &get_existing(&existing, "VM_CPU_CORES", "2"),
        );
        env_lines.push(format!("VM_CPU_CORES={}", cpu_cores));

        // RAM
        let ram_mb = prompt_with_default(
            "VM RAM in MB (1024-16384, 4096 = 4GB recommended)",
            &get_existing(&existing, "VM_RAM_MB", "4096"),
        );
        env_lines.push(format!("VM_RAM_MB={}", ram_mb));

        // Disk Size
        let disk_size = prompt_with_default(
            "VM Disk Size (e.g. 20G, 40G, 100G)",
            &get_existing(&existing, "VM_DISK_SIZE", "40G"),
        );
        env_lines.push(format!("VM_DISK_SIZE={}", disk_size));

        // Architecture
        println!();
        println!("VM Architecture:");
        println!("  1) x86_64  — Standard Intel/AMD (most Linux images)");
        println!("  2) aarch64 — ARM64 (Raspberry Pi images, Apple Silicon)");
        let arch_choice = prompt_choice("Architecture", &["1", "2"], "1")?;
        let arch = if arch_choice == "2" {
            "aarch64"
        } else {
            "x86_64"
        };
        env_lines.push(format!("VM_ARCH={}", arch));

        // Access Mode
        println!();
        println!("VM Access Mode:");
        println!("  1) shared — LLM can use VM AND host system (run commands on both)");
        println!("  2) vm     — LLM can ONLY use the VM (host system is isolated from LLM)");
        let vm_mode_choice = prompt_choice("Access mode", &["1", "2"], "1")?;
        let vm_mode = if vm_mode_choice == "2" {
            "vm"
        } else {
            "shared"
        };
        env_lines.push(format!("VM_MODE={}", vm_mode));

        // Socket Mode
        println!();
        println!("VM Communication Socket:");
        println!("  1) unix — Unix sockets (faster, ~5x less latency, Linux/macOS only)");
        println!("  2) tcp  — TCP sockets (cross-platform, works on Windows too)");
        let default_socket = if cfg!(target_os = "linux") || cfg!(target_os = "macos") {
            "1"
        } else {
            "2"
        };
        let socket_choice = prompt_choice("Socket mode", &["1", "2"], default_socket)?;
        let socket_mode = if socket_choice == "1" { "unix" } else { "tcp" };
        env_lines.push(format!("VM_SOCKET_MODE={}", socket_mode));

        // Summary
        println!();
        println!("VM Configuration Summary:");
        println!("  CPU: {} cores", cpu_cores);
        println!("  RAM: {} MB", ram_mb);
        println!("  Disk: {}", disk_size);
        println!("  Arch: {}", arch);
        println!(
            "  Mode: {} (LLM {})",
            vm_mode,
            if vm_mode == "vm" {
                "VM only"
            } else {
                "VM + host"
            }
        );
        println!(
            "  Socket: {} ({})",
            socket_mode,
            if socket_mode == "unix" {
                "fastest"
            } else {
                "cross-platform"
            }
        );
        println!();

        // Installation Disks
        println!("Installation ISOs:");
        println!("Add paths to Linux ISO files. The LLM can use these to install OSes.");
        println!("Example: /home/user/Downloads/ubuntu-24.04.iso");
        println!("Leave empty to skip. You can add more later via dashboard or context.");
        let mut iso_paths: Vec<String> = Vec::new();
        loop {
            let num = iso_paths.len() + 1;
            let prompt = if iso_paths.is_empty() {
                "ISO path (or press Enter to skip)".to_string()
            } else {
                format!("ISO path #{} (or press Enter to finish)", num)
            };
            let iso = prompt_with_default(&prompt, "");
            if iso.is_empty() {
                break;
            }
            if std::path::Path::new(&iso).exists() {
                let name = std::path::Path::new(&iso)
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                iso_paths.push(iso.clone());
                println!("  Added: {} ({})", name, iso);
            } else {
                println!("  Warning: file not found at '{}'. Adding anyway.", iso);
                iso_paths.push(iso);
            }
        }

        println!();
        println!("Secrets are injected into the VM as files (not env vars).");
        println!("The LLM cannot read secret values — apps must use: cat /run/secrets/SECRET_NAME");
        println!("The VM auto-starts when you run: ./praxis run");
        if !iso_paths.is_empty() {
            println!("Installation ISOs configured: {}", iso_paths.len());
            println!("The LLM can use vm_install to boot from these ISOs.");

            // Save ISOs to installation_disks.json
            let data_dir_for_isos = env_lines
                .iter()
                .find(|l| l.starts_with("DATA_DIR="))
                .map(|l| l.strip_prefix("DATA_DIR=").unwrap_or("./data"))
                .unwrap_or("./data");
            let vm_dir = format!("{}/vm", data_dir_for_isos);
            let _ = std::fs::create_dir_all(&vm_dir);
            let disks: Vec<serde_json::Value> = iso_paths
                .iter()
                .map(|path| {
                    let name = std::path::Path::new(path)
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    serde_json::json!({ "name": name, "path": path })
                })
                .collect();
            let disks_json =
                serde_json::to_string_pretty(&disks).unwrap_or_else(|_| "[]".to_string());
            let _ = std::fs::write(format!("{}/installation_disks.json", vm_dir), disks_json);
        }
    } else if has_vm {
        // Keep existing values
        for key in &[
            "VM_ENABLED",
            "VM_CPU_CORES",
            "VM_RAM_MB",
            "VM_DISK_SIZE",
            "VM_ARCH",
            "VM_MODE",
            "VM_SOCKET_MODE",
        ] {
            if let Some(val) = existing.get(*key) {
                env_lines.push(format!("{}={}", key, val));
            }
        }
    }
    println!();

    // Data directory
    println!("--- Data Storage ---");
    let data_dir = prompt_with_default(
        "Data Directory",
        &get_existing(&existing, "DATA_DIR", "./data"),
    );
    env_lines.push(format!("DATA_DIR={}", data_dir));
    println!();

    // Logging
    println!("--- Logging ---");
    println!("Log levels: trace, debug, info, warn, error");
    let log_level = prompt_with_default("Log Level", &get_existing(&existing, "RUST_LOG", "info"));
    env_lines.push(format!("RUST_LOG={}", log_level));
    println!();

    // Voice (optional)
    println!("--- Voice (Optional) ---");
    let has_voice =
        existing.contains_key("VOICE_STT_TYPE") || existing.contains_key("VOICE_TTS_TYPE");
    let voice_label = if has_voice {
        "Update voice config?"
    } else {
        "Setup Voice?"
    };
    let setup_voice = prompt_yes_no(voice_label, has_voice)?;

    if setup_voice {
        let existing_stt = get_existing(&existing, "VOICE_STT_TYPE", "vosk");
        println!("Speech-to-Text engine:");
        println!("  1) Vosk (local, free)");
        println!("  2) Whisper (local, free)");
        println!("  3) ElevenLabs (cloud, paid)");
        let stt_default = match existing_stt.as_str() {
            "whisper" => "2",
            "elevenlabs" => "3",
            _ => "1",
        };
        let stt_choice = prompt_choice("Select STT", &["1", "2", "3"], stt_default)?;
        let stt_type = match stt_choice.as_str() {
            "1" => "vosk",
            "2" => "whisper",
            "3" => "elevenlabs",
            _ => "vosk",
        };
        env_lines.push(format!("VOICE_STT_TYPE={}", stt_type));

        if stt_type == "vosk" {
            let model_path = prompt_with_default(
                "Vosk Model Path",
                &get_existing(&existing, "VOSK_MODEL_PATH", "./models/vosk-model-small-de"),
            );
            env_lines.push(format!("VOSK_MODEL_PATH={}", model_path));
        } else if stt_type == "whisper" {
            let model_path = prompt_with_default(
                "Whisper Model Path",
                &get_existing(&existing, "WHISPER_MODEL_PATH", "./models/ggml-tiny.en.bin"),
            );
            env_lines.push(format!("WHISPER_MODEL_PATH={}", model_path));
        } else if stt_type == "elevenlabs" {
            let api_key = prompt_required_with_existing(
                "ElevenLabs API Key",
                &get_existing(&existing, "ELEVENLABS_API_KEY", ""),
            );
            env_lines.push(format!("ELEVENLABS_API_KEY={}", api_key));
        }

        println!();
        let existing_tts = get_existing(&existing, "VOICE_TTS_TYPE", "windows_sapi");
        println!("Text-to-Speech engine:");
        println!("  1) Windows SAPI (local, free)");
        println!("  2) ElevenLabs (cloud, paid)");
        println!("  3) Qwen TTS (local, free)");
        let tts_default = match existing_tts.as_str() {
            "elevenlabs" => "2",
            "qwen_tts" => "3",
            _ => "1",
        };
        let tts_choice = prompt_choice("Select TTS", &["1", "2", "3"], tts_default)?;
        let tts_type = match tts_choice.as_str() {
            "1" => "windows_sapi",
            "2" => "elevenlabs",
            "3" => "qwen_tts",
            _ => "windows_sapi",
        };
        env_lines.push(format!("VOICE_TTS_TYPE={}", tts_type));

        if tts_type == "elevenlabs" {
            if !env_lines
                .iter()
                .any(|l| l.starts_with("ELEVENLABS_API_KEY="))
            {
                let api_key = prompt_required_with_existing(
                    "ElevenLabs API Key",
                    &get_existing(&existing, "ELEVENLABS_API_KEY", ""),
                );
                env_lines.push(format!("ELEVENLABS_API_KEY={}", api_key));
            }
            let voice_id = prompt_with_default(
                "ElevenLabs Voice ID",
                &get_existing(&existing, "ELEVENLABS_VOICE_ID", "21m00Tcm4TlvDq8ikWAM"),
            );
            env_lines.push(format!("ELEVENLABS_VOICE_ID={}", voice_id));
        } else if tts_type == "qwen_tts" {
            let server = prompt_with_default(
                "Qwen TTS Server URL",
                &get_existing(&existing, "QWEN_TTS_SERVER", "http://localhost:8001"),
            );
            env_lines.push(format!("QWEN_TTS_SERVER={}", server));
        }
    } else {
        // Keep existing voice config
        for key in &[
            "VOICE_STT_TYPE",
            "VOICE_TTS_TYPE",
            "VOSK_MODEL_PATH",
            "WHISPER_MODEL_PATH",
            "ELEVENLABS_API_KEY",
            "ELEVENLABS_VOICE_ID",
            "QWEN_TTS_SERVER",
        ] {
            if let Some(val) = existing.get(*key) {
                env_lines.push(format!("{}={}", key, val));
            }
        }
    }
    println!();

    // Write .env file
    println!("--- Saving Configuration ---");
    let env_path = ".env";

    let mut content = String::new();
    content.push_str("# Praxis AI Agent Platform Configuration\n");
    content.push_str("# Generated by: praxis onboard --interactive\n\n");

    content.push_str("# LLM Provider\n");
    for line in &env_lines {
        if line.starts_with("USE_PROVIDER=")
            || line.starts_with("OPENAI_")
            || line.starts_with("ANTHROPIC_")
            || line.starts_with("LLAMACPP_")
            || line.starts_with("OLLAMA_")
            || line.starts_with("MINIMAX_")
            || line.starts_with("MIMO_")
            || line.starts_with("OPENROUTER_")
            || line.starts_with("LLM_")
            || line.starts_with("POML_CLI=")
        {
            content.push_str(line);
            content.push('\n');
        }
    }
    content.push('\n');

    content.push_str("# Embedding Model (for RAG)\n");
    for line in &env_lines {
        if line.starts_with("EMBEDDING_") {
            content.push_str(line);
            content.push('\n');
        }
    }
    content.push('\n');

    content.push_str("# Discord\n");
    for line in &env_lines {
        if line.starts_with("DISCORD_") {
            content.push_str(line);
            content.push('\n');
        }
    }
    content.push('\n');

    content.push_str("# Gateway\n");
    for line in &env_lines {
        if line.starts_with("GATEWAY_") {
            content.push_str(line);
            content.push('\n');
        }
    }
    content.push('\n');

    content.push_str("# Dashboard\n");
    for line in &env_lines {
        if line.starts_with("DASHBOARD_") {
            content.push_str(line);
            content.push('\n');
        }
    }
    content.push('\n');

    content.push_str("# Data\n");
    for line in &env_lines {
        if line.starts_with("DATA_DIR=") || line.starts_with("RUST_LOG=") {
            content.push_str(line);
            content.push('\n');
        }
    }
    content.push('\n');

    content.push_str("# Voice\n");
    for line in &env_lines {
        if line.starts_with("VOICE_")
            || line.starts_with("VOSK_")
            || line.starts_with("WHISPER_")
            || line.starts_with("ELEVENLABS_")
            || line.starts_with("QWEN_")
        {
            content.push_str(line);
            content.push('\n');
        }
    }
    content.push('\n');

    content.push_str("# Plugins\n");
    for line in &env_lines {
        if line.starts_with("PLUGIN_") {
            content.push_str(line);
            content.push('\n');
        }
    }
    content.push('\n');

    content.push_str("# VM\n");
    for line in &env_lines {
        if line.starts_with("VM_") {
            content.push_str(line);
            content.push('\n');
        }
    }

    std::fs::write(env_path, content)?;

    println!("Configuration saved to {}", env_path);

    std::fs::create_dir_all("data")?;
    let data_dir = env_lines
        .iter()
        .find(|l| l.starts_with("DATA_DIR="))
        .map(|l| l.strip_prefix("DATA_DIR=").unwrap_or("./data"))
        .unwrap_or("./data")
        .to_string();
    std::fs::create_dir_all(&data_dir)?;
    // Plugins install under DATA_DIR by default: one directory holds every
    // installed plugin (see docs/PLUGIN_LIFECYCLE.md).
    let plugins_dir = std::env::var("PLUGINS_DIR")
        .unwrap_or_else(|_| format!("{}/plugins", data_dir.trim_end_matches('/')));
    std::fs::create_dir_all(&plugins_dir)?;
    // One complete bundle for onboarding and repair, including all relative
    // POML imports, canonical contexts/*.sm, native skills and bitmap branding.
    // Preserve existing user customizations instead of overwriting prompts.
    let assets = crate::assets::install(std::path::Path::new("."), false, false)?;
    println!("Installed {} runtime assets; preserved {} existing files.", assets.created.len(), assets.preserved.len());
    println!("For dashboard upgrades without reconfiguring: ./praxis repair-assets --update-dashboard");

    // Install the shipped plugins through the normal lifecycle: manifest
    // validation, PLUGIN_HOOKS policy and praxis.lock.json records — exactly
    // what 'praxis plugin install' does by hand.
    println!();
    println!("--- Plugins ---");
    let shipped = [
        "system_info",
        "brave_search",
        "minimax_image",
        "openrouter_image",
        "mimo_understand",
        "elevenlabs_tts",
        "comfyui",
        "sosse",
        "spotify",
        "vm",
    ];
    let available: Vec<String> = shipped
        .iter()
        .map(|name| name.to_string())
        .filter(|name| std::path::Path::new("plugins").join(name).join("plugin.json").is_file())
        .filter(|name| !std::path::Path::new(&plugins_dir).join(name).exists())
        .collect();
    if !available.is_empty()
        && prompt_yes_no(
            &format!(
                "Install the {} shipped plugins (search, image, media, Spotify, VM)?",
                available.len()
            ),
            true,
        )?
    {
        let run_hooks = crate::plugins::lifecycle::resolve_consent(
            crate::plugins::lifecycle::HookPolicy::from_env()?,
            false,
            false,
            "Run plugin install hooks?",
        );
        for name in available {
            let source = std::path::Path::new("plugins").join(&name);
            match crate::plugins::lifecycle::install(&crate::plugins::lifecycle::InstallRequest {
                source: &source,
                plugins_dir: std::path::Path::new(&plugins_dir),
                data_dir: std::path::Path::new(&data_dir),
                run_hooks,
                dry_run: false,
            })
            .await
            {
                Ok(report) => println!("  installed {}", report.name),
                Err(error) => println!("  {name} skipped: {error}"),
            }
        }
    }
    println!("Everything installed lives in one directory: {plugins_dir}");
    println!("Frontends and tool packages build from source whenever you want them:");
    println!("  praxis plugin install --build ./plugins/tui        # terminal UI");
    println!("  praxis plugin install --build ./plugins/dashboard  # web dashboard");

    // Create database
    let _db = crate::db::Database::new(std::path::Path::new(&data_dir))?;
    println!("Database initialized");

    // Save all secrets to encrypted storage (enc2)
    let secrets = crate::db::secrets::Secrets {
        discord_bot_token: env_lines
            .iter()
            .find(|l| l.starts_with("DISCORD_BOT_TOKEN="))
            .map(|l| {
                l.strip_prefix("DISCORD_BOT_TOKEN=")
                    .unwrap_or("")
                    .to_string()
            }),
        openai_api_key: env_lines
            .iter()
            .find(|l| l.starts_with("OPENAI_API_KEY="))
            .map(|l| l.strip_prefix("OPENAI_API_KEY=").unwrap_or("").to_string()),
        anthropic_api_key: env_lines
            .iter()
            .find(|l| l.starts_with("ANTHROPIC_API_KEY="))
            .map(|l| {
                l.strip_prefix("ANTHROPIC_API_KEY=")
                    .unwrap_or("")
                    .to_string()
            }),
        ollama_api_key: env_lines
            .iter()
            .find(|l| l.starts_with("OLLAMA_API_KEY="))
            .map(|l| l.strip_prefix("OLLAMA_API_KEY=").unwrap_or("").to_string()),
        llamacpp_api_key: env_lines
            .iter()
            .find(|l| l.starts_with("LLAMACPP_API_KEY="))
            .map(|l| l.strip_prefix("LLAMACPP_API_KEY=").unwrap_or("").to_string()),
        minimax_api_key: env_lines
            .iter()
            .find(|l| l.starts_with("MINIMAX_API_KEY="))
            .map(|l| l.strip_prefix("MINIMAX_API_KEY=").unwrap_or("").to_string()),
        mimo_api_key: env_lines
            .iter()
            .find(|l| l.starts_with("MIMO_API_KEY="))
            .map(|l| l.strip_prefix("MIMO_API_KEY=").unwrap_or("").to_string()),
        elevenlabs_api_key: env_lines
            .iter()
            .find(|l| l.starts_with("ELEVENLABS_API_KEY="))
            .map(|l| {
                l.strip_prefix("ELEVENLABS_API_KEY=")
                    .unwrap_or("")
                    .to_string()
            }),
        gateway_api_key: Some(gateway_api_key.clone()),
        dashboard_admin_password: Some(admin_password.clone()),
        ..Default::default()
    };
    crate::db::secrets::save_secrets(&secrets, &master_password)?;
    println!("Secrets encrypted and saved to secrets.enc2");

    // Remove sensitive values from .env since they are now in enc2
    let env_path = ".env";
    let env_content = std::fs::read_to_string(env_path)?;
    let filtered: String = env_content
        .lines()
        .filter(|line| {
            !line.starts_with("OPENAI_API_KEY=")
                && !line.starts_with("ANTHROPIC_API_KEY=")
                && !line.starts_with("LLAMACPP_API_KEY=")
                && !line.starts_with("MINIMAX_API_KEY=")
                && !line.starts_with("MIMO_API_KEY=")
                && !line.starts_with("ELEVENLABS_API_KEY=")
                && !line.starts_with("DISCORD_BOT_TOKEN=")
                && !line.starts_with("GATEWAY_API_KEY=")
                && !line.starts_with("DASHBOARD_ADMIN_PASSWORD=")
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(env_path, filtered)?;
    println!("Sensitive values removed from .env (stored in secrets.enc2 instead)");

    println!();
    println!("========================================");
    println!("   Setup Complete!");
    println!("========================================");
    println!();
    println!("Next steps:");
    println!("  1. Review your .env file");
    println!("  2. Run: praxis run");
    println!("  3. Enter your MASTER_KEY when prompted to unlock secrets");
    println!();

    Ok(())
}

fn prompt(message: &str) -> anyhow::Result<String> {
    print!("{}: ", message);
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
}

fn prompt_required(message: &str) -> anyhow::Result<String> {
    loop {
        let value = prompt(message)?;
        if !value.is_empty() {
            return Ok(value);
        }
        println!("  This field is required.");
    }
}

fn prompt_with_default(message: &str, default: &str) -> String {
    print!("{} [{}]: ", message, default);
    io::stdout().flush().unwrap();
    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    let trimmed = input.trim();
    if trimmed.is_empty() {
        default.to_string()
    } else {
        trimmed.to_string()
    }
}

fn prompt_with_default_or_generate(message: &str, length: usize, existing: &str) -> String {
    if !existing.is_empty() {
        let masked = if existing.len() > 8 {
            format!("{}...", &existing[..8])
        } else {
            existing.to_string()
        };
        print!("{} [{}]: ", message, masked);
        io::stdout().flush().unwrap();
        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();
        let trimmed = input.trim();
        if trimmed.is_empty() {
            existing.to_string()
        } else {
            trimmed.to_string()
        }
    } else {
        print!("{} [press Enter to generate]: ", message);
        io::stdout().flush().unwrap();
        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();
        let trimmed = input.trim();
        if trimmed.is_empty() {
            generate_random_key(length)
        } else {
            trimmed.to_string()
        }
    }
}

fn prompt_password(message: &str) -> anyhow::Result<String> {
    loop {
        let password = rpassword::prompt_password(format!("{}: ", message))?;
        if password.len() < 8 {
            println!("  Password must be at least 8 characters.");
            continue;
        }
        let confirm = rpassword::prompt_password("Confirm password: ")?;
        if password == confirm {
            return Ok(password);
        }
        println!("  Passwords do not match. Try again.");
    }
}

fn prompt_yes_no(message: &str, default: bool) -> anyhow::Result<bool> {
    let default_str = if default { "Y/n" } else { "y/N" };
    print!("{} [{}]: ", message, default_str);
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim().to_lowercase();
    if trimmed.is_empty() {
        Ok(default)
    } else {
        Ok(trimmed == "y" || trimmed == "yes")
    }
}

fn prompt_choice(message: &str, options: &[&str], default: &str) -> anyhow::Result<String> {
    loop {
        print!("{} [{}]: ", message, default);
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Ok(default.to_string());
        }
        if options.contains(&trimmed) {
            return Ok(trimmed.to_string());
        }
        println!("  Invalid choice. Please select one of: {:?}", options);
    }
}

fn prompt_required_with_existing(message: &str, existing: &str) -> String {
    if !existing.is_empty() {
        let masked = if existing.len() > 8 {
            format!("{}...", &existing[..8])
        } else {
            existing.to_string()
        };
        print!("{} [{}]: ", message, masked);
        io::stdout().flush().unwrap();
        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();
        let trimmed = input.trim();
        if trimmed.is_empty() {
            existing.to_string()
        } else {
            trimmed.to_string()
        }
    } else {
        prompt_required(message).unwrap_or_default()
    }
}

fn prompt_password_with_existing(message: &str, existing: &str) -> anyhow::Result<String> {
    if !existing.is_empty() {
        print!("{} [press Enter to keep existing]: ", message);
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Ok(existing.to_string());
        }
    }
    prompt_password(message)
}

fn load_existing_env() -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    if let Ok(content) = std::fs::read_to_string(".env") {
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                map.insert(key.trim().to_string(), value.trim().to_string());
            }
        }
    }
    map
}

fn get_existing(
    existing: &std::collections::HashMap<String, String>,
    key: &str,
    fallback: &str,
) -> String {
    existing
        .get(key)
        .cloned()
        .unwrap_or_else(|| fallback.to_string())
}

fn persist_provider_selection(lines: &mut Vec<String>, provider: &str) {
    lines.retain(|line| !line.starts_with("USE_PROVIDER="));
    lines.push(format!("USE_PROVIDER={provider}"));
}

fn select_provider(
    choice: &str,
    existing: &std::collections::HashMap<String, String>,
    env_lines: &mut Vec<String>,
) -> anyhow::Result<String> {
    match choice {
        "1" => {
            let api_key = prompt_required_with_existing(
                "OpenAI API Key",
                &get_existing(existing, "OPENAI_API_KEY", ""),
            );
            let model = prompt_with_default(
                "OpenAI Model",
                &get_existing(existing, "OPENAI_MODEL", "gpt-4o"),
            );
            let base_url = prompt_with_default(
                "OpenAI API Base URL",
                &get_existing(existing, "OPENAI_API_BASE", "https://api.openai.com/v1"),
            );
            env_lines.push(format!("OPENAI_API_KEY={}", api_key));
            env_lines.push(format!("OPENAI_MODEL={}", model));
            env_lines.push(format!("OPENAI_API_BASE={}", base_url));
            Ok("openai".to_string())
        }
        "2" => {
            let api_key = prompt_required_with_existing(
                "Anthropic API Key",
                &get_existing(existing, "ANTHROPIC_API_KEY", ""),
            );
            let model = prompt_with_default(
                "Anthropic Model",
                &get_existing(existing, "ANTHROPIC_MODEL", "claude-3-5-sonnet-20241022"),
            );
            let base_url = prompt_with_default(
                "Anthropic API Base URL",
                &get_existing(existing, "ANTHROPIC_API_BASE", "https://api.anthropic.com"),
            );
            env_lines.push(format!("ANTHROPIC_API_KEY={}", api_key));
            env_lines.push(format!("ANTHROPIC_MODEL={}", model));
            env_lines.push(format!("ANTHROPIC_API_BASE={}", base_url));
            Ok("anthropic".to_string())
        }
        "3" => {
            let base_url = prompt_with_default(
                "Llama.cpp Server API Base URL",
                &get_existing(existing, "LLAMACPP_API_BASE", "http://localhost:8080"),
            );
            let model = prompt_with_default(
                "Llama.cpp Model",
                &get_existing(existing, "LLAMACPP_MODEL", "llama.cpp"),
            );
            let api_key = prompt_with_default(
                "Llama.cpp API Key (optional, for authenticated servers)",
                &get_existing(existing, "LLAMACPP_API_KEY", ""),
            );
            env_lines.push(format!("LLAMACPP_API_BASE={}", base_url));
            env_lines.push(format!("LLAMACPP_MODEL={}", model));
            if !api_key.is_empty() {
                env_lines.push(format!("LLAMACPP_API_KEY={}", api_key));
            }
            Ok("llamacpp".to_string())
        }
        "4" => {
            let base_url = prompt_with_default(
                "Ollama API Base URL",
                &get_existing(existing, "OLLAMA_API_BASE", "http://localhost:11434"),
            );
            let model = prompt_with_default(
                "Ollama Model",
                &get_existing(existing, "OLLAMA_MODEL", "llama3"),
            );
            let api_key = prompt_with_default(
                "Ollama API Key (optional, for Ollama Cloud)",
                &get_existing(existing, "OLLAMA_API_KEY", ""),
            );
            env_lines.push(format!("OLLAMA_API_BASE={}", base_url));
            env_lines.push(format!("OLLAMA_MODEL={}", model));
            if !api_key.is_empty() {
                env_lines.push(format!("OLLAMA_API_KEY={}", api_key));
            }
            Ok("ollama".to_string())
        }
        "5" => {
            let api_key = prompt_required_with_existing(
                "MiniMax API Key",
                &get_existing(existing, "MINIMAX_API_KEY", ""),
            );
            let model = prompt_with_default(
                "MiniMax Model",
                &get_existing(existing, "MINIMAX_MODEL", "MiniMax-Text-01"),
            );
            let base_url = prompt_with_default(
                "MiniMax API Base URL",
                &get_existing(existing, "MINIMAX_API_BASE", "https://api.minimax.chat/v1"),
            );
            let existing_mode = get_existing(existing, "MINIMAX_API_MODE", "openai");
            println!("  API compatibility mode:");
            println!("    1) OpenAI-compatible (default)");
            println!("    2) Anthropic-compatible");
            let mode_default = if existing_mode == "anthropic" {
                "2"
            } else {
                "1"
            };
            let api_mode = prompt_choice("Select API mode", &["1", "2"], mode_default)?;
            let mode_str = if api_mode == "2" {
                "anthropic"
            } else {
                "openai"
            };
            env_lines.push(format!("MINIMAX_API_KEY={}", api_key));
            env_lines.push(format!("MINIMAX_MODEL={}", model));
            env_lines.push(format!("MINIMAX_API_BASE={}", base_url));
            env_lines.push(format!("MINIMAX_API_MODE={}", mode_str));
            Ok("minimax".to_string())
        }
        "6" => {
            let api_key = prompt_required_with_existing(
                "MiMo API Key",
                &get_existing(existing, "MIMO_API_KEY", ""),
            );
            let model = prompt_with_default(
                "MiMo Model",
                &get_existing(existing, "MIMO_MODEL", "mimo-v2.5-pro"),
            );
            let base_url = prompt_with_default(
                "MiMo API Base URL",
                &get_existing(existing, "MIMO_API_BASE", "https://api.xiaomimimo.com/v1"),
            );
            let existing_mode = get_existing(existing, "MIMO_API_MODE", "openai");
            println!("  API compatibility mode:");
            println!("    1) OpenAI-compatible (default)");
            println!("    2) Anthropic-compatible");
            let mode_default = if existing_mode == "anthropic" {
                "2"
            } else {
                "1"
            };
            let api_mode = prompt_choice("Select API mode", &["1", "2"], mode_default)?;
            let mode_str = if api_mode == "2" {
                "anthropic"
            } else {
                "openai"
            };
            env_lines.push(format!("MIMO_API_KEY={}", api_key));
            env_lines.push(format!("MIMO_MODEL={}", model));
            env_lines.push(format!("MIMO_API_BASE={}", base_url));
            env_lines.push(format!("MIMO_API_MODE={}", mode_str));
            Ok("mimo".to_string())
        }
        _ => Ok("openai".to_string()),
    }
}

fn generate_random_key(length: usize) -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let chars: Vec<char> = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
        .chars()
        .collect();
    let key: String = (0..length)
        .map(|_| chars[rng.gen_range(0..chars.len())])
        .collect();
    println!("  Generated key: {}", key);
    key
}

#[cfg(test)]
mod security_tests {
    use super::*;

    #[test]
    fn resilience_onboarding_persists_provider_exactly_once() {
        for initial in [vec![], vec!["USE_PROVIDER=openai".into()]] {
            let mut lines = initial;
            persist_provider_selection(&mut lines, "ollama");
            persist_provider_selection(&mut lines, "ollama");
            assert_eq!(lines, vec!["USE_PROVIDER=ollama"]);
        }
    }

    #[test]
    fn test_generate_random_key() {
        let key = generate_random_key(32);
        assert_eq!(key.len(), 32);
    }

    #[test]
    fn test_generate_random_key_different() {
        let key1 = generate_random_key(16);
        let key2 = generate_random_key(16);
        assert_ne!(key1, key2);
    }
}
