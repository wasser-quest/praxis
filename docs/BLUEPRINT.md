# Praxis — Complete Blueprint

> Single source of truth for rebuilding the AI Agent Platform.
> Version: 0.2.0
> Every piece of code, every design decision, every test.
> Start here. Everything else is reference.

---

## Ordnerstruktur

```
praxis/                           ← NEUES Projekt (hier implementieren)
├── Cargo.toml                    ← Dependencies (fertig)
├── src/                          ← HIER CODE SCHREIBEN
├── docs/
│   ├── AGENTS.md                 ← Agent-Instructions
│   └── BLUEPRINT.md              ← DIESE DATEI (alles drin)
├── reference/                    ← ALTER CODE (nur zum Nachschlagen)
│   └── src/                      ← Kopie des alten Projekts (zum Nachschlagen)
│       ├── sm.rs                 ← Statemachine Parser
│       ├── tags.rs               ← §-Tag Parser
│       ├── event_channel.rs      ← Event Bus
│       ├── gateway/              ← Gateway (alt)
│       ├── discord/              ← Discord Bot (alt)
│       ├── voice/                ← Voice System (alt)
│       ├── tools/                ← Tools (alt)
│       ├── db/                   ← Database (alt)
│       └── dashboard/            ← Dashboard (alt)
├── templates/                    ← POML Templates
├── contexts/                     ← SM Workflows
├── migrations/                   ← SQL Migrations
├── skills/                       ← User Skills (NEU)
└── plugins/                      ← Plugins (NEU)
```

**Wichtig:**
- `src/` = HIER implementieren (neu, sauber, mit Tests)
- `reference/` = Alter Code zum Nachschlagen (Voice, Discord, etc.)
- `docs/BLUEPRINT.md` = Diese Datei — alles was du brauchst

---

## Table of Contents

1. [Design](#1-design)
2. [Architecture](#2-architecture)
3. [Cargo.toml](#3-cargotoml)
4. [Configuration](#4-configuration)
5. [Database Layer](#5-database-layer)
6. [Statemachine](#6-context-language)
7. [Tags System](#7-tags-system)
8. [Event Channel](#8-event-channel)
9. [Gateway](#9-gateway)
10. [LLM Client](#10-llm-client)
11. [POML Renderer](#11-poml-renderer)
12. [Discord Integration](#12-discord-integration)
13. [Voice System](#13-voice-system)
14. [Tools](#14-tools)
15. [Dashboard](#15-dashboard)
16. [Skills System](#16-skills-system)
17. [Plugin System](#17-plugin-system)
18. [RAG & Web Search](#18-rag--web-search)
19. [Cron Jobs](#19-cron-jobs)
20. [Tests](#20-tests)

---

# 1. Design

## Visual Identity

```
Brand:      #23594D (Deep Green)
Signal:     #C58938 (Ochre checkpoint)
Homepage:   #F5F4EE (Paper)
Ink:        #142724
Dashboard:  #101C19 (Dark), #F5F4EE (Light)
Surface:    #1B2C25 (Dark), #FFFEF9 (Light)
Text:       #EBEEE5 (Dark), #142724 (Light)
Success:    #82C8AD (Dark), #23664C (Light)
Warning:    #E1B36A (Dark), #805114 (Light)
Error:      #EF7B73 (Dark), #B53835 (Light)
```

## Typography

```
Homepage headers: Georgia, Times New Roman, serif
Dashboard / body: System sans-serif
Code:             System monospace (Consolas, Liberation Mono)
```

## Dashboard Layout

```
┌─────────────────────────────────────────────────────────────────────┐
│  ⚡ PRAXIS v0.2                              [Status: ● Online]    │
├──────────┬──────────────────────────────────────────────────────────┤
│          │                                                          │
│  📊 Dash │  ┌────────────────────────────────────────────────────┐  │
│  💬 Chat │  │  Live Event Stream                                 │  │
│  🤖 Agent│  │  ────────────────────────────────────────────────  │  │
│  🎤 Voice│  │  12:34:01  User: "Hello bot"                      │  │
│  📁 Files│  │  12:34:02  Agent: Thinking...                     │  │
│  🔧 Tools│  │  12:34:05  Tool: execute_terminal("ls -la")       │  │
│  📝 Templ│  │  12:34:06  Tool Result: 24 files...               │  │
│  🔑 Secr │  │  12:34:08  Agent: "Here are your files..."        │  │
│  ⚙️ Sett │  │  12:34:08  TTS: Generating audio...               │  │
│  📅 Cron │  │  12:34:10  TTS: Playing in voice channel          │  │
│  🧩 Plug │  └────────────────────────────────────────────────────┘  │
│  📚 Docs │                                                          │
│          │  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  │
│          │  │ Active Users │  │ LLM Calls/h  │  │ Uptime       │  │
│          │  │     12       │  │     847      │  │  2h 34m      │  │
│          │  └──────────────┘  └──────────────┘  └──────────────┘  │
│          │                                                          │
└──────────┴──────────────────────────────────────────────────────────┘
```

## Design Principles

1. **Dark Mode First** — Alles ist dunkel, helle Akzente
2. **Glassmorphism** — Blur-Effekte, Transparenz
3. **Monospace für Code** — JetBrains Mono überall wo Code ist
4. **Cards** — Alles in Cards mit abgerundeten Ecken
5. **Gradient Accents** — Purple → Cyan Gradient für Highlights
6. **Animations** — Smooth Transitions, Hover-Effekte
7. **Responsive** — Funktioniert auf Desktop und Tablet

---

# 2. Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                         Clients                                       │
│  ┌──────────┐  ┌───────────┐  ┌──────────┐  ┌────────────────────┐  │
│  │ Discord  │  │ Dashboard │  │ CLI      │  │ Cron/Webhook       │  │
│  │ Bot      │  │ (Web UI)  │  │ (praxis) │  │ (Agent Scheduler)  │  │
│  └────┬─────┘  └─────┬─────┘  └────┬─────┘  └─────────┬──────────┘  │
└───────┼──────────────┼──────────────┼──────────────────┼─────────────┘
        │              │              │                  │
        ▼              ▼              ▼                  ▼
┌──────────────────────────────────────────────────────────────────────┐
│                    Gateway (Axum :3537)                               │
│  ┌────────┐  ┌────────┐  ┌──────────┐  ┌──────────────────────────┐  │
│  │ Auth   │  │ Rate   │  │ POML     │  │ LLM Router               │  │
│  │ (JWT)  │  │ Limit  │  │ Renderer │  │ (OpenAI, Anthropic,      │  │
│  └────────┘  └────────┘  └──────────┘  │  Ollama, MiniMax, MiMo)  │  │
│                                         └────────────┬─────────────┘  │
│  ┌──────────────────────────────────────────────────┐│               │
│  │                Tool Executor                      ││               │
│  │  ┌────────┐ ┌────────┐ ┌────────┐ ┌────────────┐││               │
│  │  │Terminal│ │Files   │ │RAG     │ │Web Search  │││               │
│  │  │Sandbox │ │Sandbox │ │Vector  │ │DuckDuckGo  │││               │
│  │  └────────┘ └────────┘ └────────┘ └────────────┘││               │
│  │  ┌────────┐ ┌────────┐ ┌────────┐ ┌────────────┐││               │
│  │  │Discord │ │Image   │ │Plugins │ │Skills      │││               │
│  │  │Upload  │ │AI      │ │(HTTP)  │ │(POML)      │││               │
│  │  └────────┘ └────────┘ └────────┘ └────────────┘││               │
│  └──────────────────────────────────────────────────┘│               │
│                                                      │               │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐               │
│  │ Agent Loop   │  │ Cron         │  │ Event Bus    │               │
│  │ (Tool Calls) │  │ Scheduler    │  │ (broadcast)  │               │
│  └──────────────┘  └──────────────┘  └──────────────┘               │
└──────────────────────────────────────────────────────────────────────┘
        │                 │                    │
        ▼                 ▼                    ▼
┌──────────────┐  ┌──────────────┐  ┌──────────────────┐
│ SQLite       │  │ Vector Store │  │ File Storage     │
│ (WAL Mode)   │  │ (sqlite-vec) │  │ (Templates,      │
│              │  │              │  │  Skills, Plugins)│
└──────────────┘  └──────────────┘  └──────────────────┘
```

## Data Flow

```
Discord User → handler.rs → ws_client → Gateway WS → message_handler.rs
    → Context laden (SQLite)
    → POML Template rendern (Single File)
    → LLM Router → Provider (OpenAI/Anthropic/Ollama/MiniMax/MiMo)
    → Tool Calls → Tool Executor
    → Response → Discord (Wait-Then-Send)
```

## File Structure

```
praxis/
├── Cargo.toml
├── .env
├── src/
│   ├── main.rs                      # CLI, Startet alle Services
│   ├── lib.rs                       # Public API
│   │
│   ├── gateway/
│   │   ├── mod.rs                   # Axum Server, Router, GatewayState
│   │   ├── ws_handler.rs            # WebSocket Handler
│   │   ├── http_handler.rs          # REST API (NEU)
│   │   ├── auth.rs                  # JWT + API-Key Auth (NEU)
│   │   ├── rate_limiter.rs          # Rate Limiting (NEU)
│   │   ├── message_handler.rs       # Message Pipeline
│   │   ├── agent_loop.rs            # Agent Loop (extrahiert) (NEU)
│   │   ├── llm/
│   │   │   ├── mod.rs               # LLM Router (NEU)
│   │   │   ├── provider.rs          # Provider Trait (NEU)
│   │   │   ├── openai.rs            # OpenAI Provider (NEU)
│   │   │   ├── anthropic.rs         # Anthropic Provider (NEU)
│   │   │   ├── ollama.rs            # Ollama Provider (NEU)
│   │   │   ├── minimax.rs           # MiniMax Provider
│   │   │   └── mimo.rs              # MiMo Provider
│   │   ├── poml.rs                  # POML Renderer
│   │   ├── templates.rs             # Template Management
│   │   └── cron_scheduler.rs        # Cron Jobs (NEU)
│   │
│   ├── db/
│   │   ├── mod.rs                   # SQLite Pool + Migrations
│   │   ├── contexts.rs              # Context CRUD
│   │   ├── messages.rs              # Message CRUD
│   │   ├── memory.rs                # Memory CRUD
│   │   ├── secrets.rs               # Secret Store (encrypted)
│   │   ├── pairings.rs              # Pairing CRUD
│   │   ├── templates.rs             # Template CRUD (NEU)
│   │   ├── cron_jobs.rs             # Cron Job CRUD (NEU)
│   │   ├── vector_store.rs          # Vector Store (NEU)
│   │   └── enc2.rs                  # AES-256-GCM Encryption
│   │
│   ├── discord/
│   │   ├── mod.rs                   # Serenity Client
│   │   ├── handler.rs               # Event Handler
│   │   ├── commands.rs              # Slash Commands
│   │   └── ws_client.rs             # WebSocket Client
│   │
│   ├── tools/
│   │   ├── mod.rs                   # Tool Registry + Dispatch
│   │   ├── registry.rs              # Dynamic Registration (NEU)
│   │   ├── execute_terminal.rs      # Terminal (Sandboxed)
│   │   ├── write_file.rs            # File Write
│   │   ├── edit_file.rs             # File Edit
│   │   ├── web_search.rs            # Web Search (NEU)
│   │   ├── rag_query.rs             # RAG Query (NEU)
│   │   ├── rag_ingest.rs            # RAG Ingest (NEU)
│   │   ├── image_analyze.rs         # Image Analysis
│   │   ├── image_generate.rs        # Image Generation
│   │   ├── discord_upload.rs        # File Upload
│   │   ├── discord_send_message.rs  # Send Message
│   │   ├── agent_control.rs         # Agent Signals
│   │   ├── context_tools.rs         # Set/Delete Context
│   │   ├── get_context.rs           # Get Context
│   │   └── learning.rs              # Memory Learning
│   │
│   ├── voice/
│   │   ├── mod.rs                   # TTS/STT Manager
│   │   └── handler.rs               # Voice Event Handler
│   │
│   ├── dashboard/
│   │   ├── mod.rs                   # Axum Server
│   │   ├── routes.rs                # REST Routes
│   │   ├── auth.rs                  # Dashboard Auth (NEU)
│   │   └── static/                  # HTML/JS/CSS
│   │
│   ├── skills/                      # Skills System (NEU)
│   │   ├── mod.rs                   # Skill Registry
│   │   └── executor.rs              # Skill Execution
│   │
│   ├── plugins/                     # Plugin System (NEU)
│   │   └── mod.rs                   # Plugin Registry + Execution
│   │
│   ├── sm.rs                        # Statemachine Parser
│   ├── tags.rs                      # §-Tag Parser
│   ├── event_channel.rs             # Event Bus
│   └── config.rs                    # Configuration (NEU)
│
├── templates/                       # POML Templates
│   ├── system.poml
│   ├── user.poml
│   ├── tools.poml
│   ├── roles/
│   ├── tasks/
│   ├── skills/
│   └── snippets/
│
├── contexts/                        # SM Workflow-Dateien
│   ├── default.cl
│   ├── chat.cl
│   ├── coding.cl
│   └── self_learning.cl
│
├── skills/                          # User Skills
│   ├── code_review/
│   ├── web_research/
│   └── translate/
│
├── plugins/                         # Plugin-Definitionen
│   ├── weather.json
│   └── system_info.json
│
├── migrations/                      # SQL Migrations
│   ├── 001_initial.sql
│   ├── 002_cron_jobs.sql
│   └── 003_vector_store.sql
│
└── docs/                            # Dokumentation
```

---

# 3. Cargo.toml

```toml
[package]
name = "praxis"
version = "0.2.0"
edition = "2021"
description = "AI Agent Platform — Praxis"

[features]
default = []
voice_vosk = ["vosk"]
voice_whisper = ["whisper-rs"]
songbird = ["dep:songbird"]
vosk = ["dep:vosk"]

[dependencies]
tokio = { version = "1", features = ["full"] }
tokio-tungstenite = "0.24"
futures-util = "0.3"
futures = "0.3"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
reqwest = { version = "0.12", features = ["json", "stream"] }
tracing = "0.1"
tracing-attributes = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }
anyhow = "1"
serenity = { version = "0.12", features = ["voice"] }
axum = { version = "0.7", features = ["ws", "json"] }
tokio-stream = "0.1"
tempfile = "3"
tokio-util = { version = "0.7", features = ["codec"] }
uuid = { version = "1", features = ["v4"] }
chrono = { version = "0.4", features = ["serde"] }
thiserror = "1"
clap = { version = "4", features = ["derive"] }
dotenvy = "0.15"
once_cell = "1"
regex = "1"
base64 = "0.22"
lazy_static = "1"
async-trait = "0.1"
dashmap = "6"
aes-gcm = "0.10"
argon2 = "0.5"
rand = "0.8"
rpassword = "7"
shlex = "1.3"
urlencoding = "2.1"
jsonwebtoken = "9"
bcrypt = "0.16"
tower-http = { version = "0.6", features = ["cors", "auth"] }
governor = "0.8"
cron = "0.15"
rusqlite = { version = "0.32", features = ["bundled", "vtab"] }
backon = "1"

vosk = { version = "0.3", optional = true }
whisper-rs = { version = "0.1", optional = true }
songbird = { version = "0.6", optional = true, features = ["driver", "builtin-queue", "serenity", "receive"] }
symphonia = { version = "0.5", features = ["all"] }
minimp3 = "0.5"
hound = "3.5"

[dev-dependencies]
wiremock = "0.6"
tempfile = "3"
```

---

# 4. Configuration

## .env

```env
# POML
POML_CLI=./poml/js/cli.cjs

# LLM Provider
USE_PROVIDER=openai
OPENAI_API_KEY=sk-...
OPENAI_MODEL=gpt-4o
OPENAI_API_BASE=https://api.openai.com/v1

# Fallback
OLLAMA_API_BASE=http://localhost:11434
OLLAMA_MODEL=llama3

# Discord
DISCORD_APPLICATION_ID=123456789

# Gateway
GATEWAY_PORT=3537
GATEWAY_API_KEY=your-api-key-here

# Dashboard
DASHBOARD_PORT=1337
DASHBOARD_ADMIN_PASSWORD=your-admin-password

# Data
DATA_DIR=./data

# Voice (optional)
# VOSK_MODEL_PATH=./models/vosk-model-small-de
# WHISPER_MODEL_PATH=./models/ggml-tiny.en.bin
# ELEVENLABS_API_KEY=...
# QWEN_TTS_SERVER=http://localhost:8001
# RVC_SERVER=http://localhost:8000

# Logging
RUST_LOG=info
```

## Config Struct

```rust
// src/config.rs

use std::env;

#[derive(Clone, Debug)]
pub struct Config {
    pub poml_cli: String,
    pub use_provider: String,
    pub openai_api_key: Option<String>,
    pub openai_model: String,
    pub openai_api_base: String,
    pub ollama_api_base: String,
    pub ollama_model: String,
    pub gateway_port: u16,
    pub gateway_api_key: String,
    pub dashboard_port: u16,
    pub dashboard_admin_password: String,
    pub data_dir: String,
    pub rust_log: String,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            poml_cli: env::var("POML_CLI").unwrap_or_else(|_| "./poml/js/cli.cjs".to_string()),
            use_provider: env::var("USE_PROVIDER").unwrap_or_else(|_| "openai".to_string()),
            openai_api_key: env::var("OPENAI_API_KEY").ok(),
            openai_model: env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-4o".to_string()),
            openai_api_base: env::var("OPENAI_API_BASE").unwrap_or_else(|_| "https://api.openai.com/v1".to_string()),
            ollama_api_base: env::var("OLLAMA_API_BASE").unwrap_or_else(|_| "http://localhost:11434".to_string()),
            ollama_model: env::var("OLLAMA_MODEL").unwrap_or_else(|_| "llama3".to_string()),
            gateway_port: env::var("GATEWAY_PORT").unwrap_or_else(|_| "3537".to_string()).parse().unwrap_or(3537),
            gateway_api_key: env::var("GATEWAY_API_KEY").unwrap_or_else(|_| {
                let key = uuid::Uuid::new_v4().to_string();
                tracing::warn!("GATEWAY_API_KEY not set, generated: {}", key);
                key
            }),
            dashboard_port: env::var("DASHBOARD_PORT").unwrap_or_else(|_| "1337".to_string()).parse().unwrap_or(1337),
            dashboard_admin_password: env::var("DASHBOARD_ADMIN_PASSWORD").unwrap_or_else(|_| {
                tracing::warn!("DASHBOARD_ADMIN_PASSWORD not set, using default 'admin'");
                "admin".to_string()
            }),
            data_dir: env::var("DATA_DIR").unwrap_or_else(|_| "./data".to_string()),
            rust_log: env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string()),
        }
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.gateway_api_key.len() < 16 {
            anyhow::bail!("GATEWAY_API_KEY must be at least 16 characters");
        }
        if self.dashboard_admin_password.len() < 8 {
            anyhow::bail!("DASHBOARD_ADMIN_PASSWORD must be at least 8 characters");
        }
        Ok(())
    }
}
```

---

# 5. Database Layer

## Schema (SQLite)

```sql
-- migrations/001_initial.sql

PRAGMA journal_mode=WAL;
PRAGMA foreign_keys=ON;

CREATE TABLE IF NOT EXISTS contexts (
    user_id TEXT PRIMARY KEY,
    data TEXT NOT NULL,
    updated_at TEXT DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    tool_call_id TEXT,
    created_at TEXT DEFAULT (datetime('now')),
    FOREIGN KEY (user_id) REFERENCES contexts(user_id)
);

CREATE TABLE IF NOT EXISTS templates (
    name TEXT PRIMARY KEY,
    content TEXT NOT NULL,
    description TEXT,
    is_system INTEGER DEFAULT 0,
    updated_at TEXT DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS pairings (
    user_id TEXT PRIMARY KEY,
    discord_user_id TEXT NOT NULL UNIQUE,
    discord_guild_id TEXT,
    paired_at TEXT DEFAULT (datetime('now')),
    last_seen_at TEXT
);

CREATE TABLE IF NOT EXISTS pending_pairings (
    code TEXT PRIMARY KEY,
    discord_user_id TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    created_at TEXT DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS tools (
    name TEXT PRIMARY KEY,
    description TEXT,
    parameters TEXT NOT NULL,
    is_enabled INTEGER DEFAULT 1,
    updated_at TEXT DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS secrets (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TEXT DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_messages_user ON messages(user_id);
CREATE INDEX IF NOT EXISTS idx_messages_created ON messages(created_at);
CREATE INDEX IF NOT EXISTS idx_pairings_discord ON pairings(discord_user_id);
```

```sql
-- migrations/002_cron_jobs.sql

CREATE TABLE IF NOT EXISTS cron_jobs (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT,
    schedule TEXT NOT NULL,
    timezone TEXT DEFAULT 'UTC',
    user_id TEXT NOT NULL,
    channel_id TEXT,
    template TEXT NOT NULL,
    prompt TEXT NOT NULL,
    context_overrides TEXT,
    enabled INTEGER DEFAULT 1,
    trigger_type TEXT DEFAULT 'cron',
    webhook_secret TEXT,
    event_type TEXT,
    last_run TEXT,
    next_run TEXT,
    run_count INTEGER DEFAULT 0,
    last_error TEXT,
    created_at TEXT DEFAULT (datetime('now')),
    updated_at TEXT DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_cron_next_run ON cron_jobs(next_run) WHERE enabled = 1;
CREATE INDEX IF NOT EXISTS idx_cron_user ON cron_jobs(user_id);
```

```sql
-- migrations/003_vector_store.sql

CREATE TABLE IF NOT EXISTS documents (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    filename TEXT NOT NULL,
    file_type TEXT NOT NULL,
    file_size INTEGER,
    chunk_count INTEGER,
    status TEXT DEFAULT 'processing',
    error_message TEXT,
    created_at TEXT DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS document_chunks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    document_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    chunk_index INTEGER NOT NULL,
    content TEXT NOT NULL,
    embedding BLOB,
    metadata TEXT,
    token_count INTEGER,
    created_at TEXT DEFAULT (datetime('now')),
    FOREIGN KEY (document_id) REFERENCES documents(id)
);

CREATE INDEX IF NOT EXISTS idx_chunks_user ON document_chunks(user_id);
CREATE INDEX IF NOT EXISTS idx_chunks_doc ON document_chunks(document_id);
CREATE INDEX IF NOT EXISTS idx_docs_user ON documents(user_id);
```

## DB Module

```rust
// src/db/mod.rs

pub mod contexts;
pub mod messages;
pub mod memory;
pub mod pairings;
pub mod secrets;
pub mod templates;
pub mod tools;
pub mod enc2;
pub mod logs;
pub mod cron_jobs;
pub mod vector_store;

use rusqlite::Connection;
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
    pub data_dir: String,
}

impl Database {
    pub fn new(data_dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(data_dir)?;

        let db_path = data_dir.join("praxis.db");
        let conn = Connection::open(&db_path)?;

        // WAL Mode für bessere Concurrency
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
            data_dir: data_dir.to_string_lossy().to_string(),
        };

        db.run_migrations()?;
        Ok(db)
    }

    fn run_migrations(&self) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();

        // Get current version
        let version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;

        if version < 1 {
            conn.execute_batch(include_str!("../../migrations/001_initial.sql"))?;
            conn.pragma_update(None, "user_version", 1)?;
        }
        if version < 2 {
            conn.execute_batch(include_str!("../../migrations/002_cron_jobs.sql"))?;
            conn.pragma_update(None, "user_version", 2)?;
        }
        if version < 3 {
            conn.execute_batch(include_str!("../../migrations/003_vector_store.sql"))?;
            conn.pragma_update(None, "user_version", 3)?;
        }

        Ok(())
    }

    pub fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap()
    }

    pub async fn ping(&self) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("SELECT 1")?;
        Ok(())
    }

    pub fn backup(&self, dest: &Path) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "VACUUM INTO ?1",
            rusqlite::params![dest.to_string_lossy()],
        )?;
        Ok(())
    }
}
```

## Contexts

```rust
// src/db/contexts.rs

use serde::{Deserialize, Serialize};
use super::Database;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Context {
    pub user_id: String,
    #[serde(default)]
    pub turn: i32,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub user_name: Option<String>,
    #[serde(default)]
    pub cl_file: Option<String>,
    #[serde(default)]
    pub active_state: Option<String>,
    #[serde(default)]
    pub active_templates: Vec<String>,
    #[serde(default)]
    pub settings: ContextSettings,
    #[serde(default, skip_serializing)]
    pub custom_data: serde_json::Value,
}

fn default_mode() -> String { "agent".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSettings {
    #[serde(default)]
    pub voice_enabled: bool,
    #[serde(default = "default_stt")]
    pub voice_stt_type: String,
    #[serde(default)]
    pub voice_vosk_model_path: Option<String>,
    #[serde(default)]
    pub voice_whisper_model_path: Option<String>,
    #[serde(default)]
    pub voice_elevenlabs_stt_api_key: Option<String>,
    #[serde(default)]
    pub voice_last_input: Option<String>,
    #[serde(default = "default_listen_timeout")]
    pub voice_listen_timeout_secs: i32,
    #[serde(default)]
    pub voice_owner_id: Option<String>,
    #[serde(default)]
    pub voice_tts_enabled: bool,
    #[serde(default = "default_tts")]
    pub voice_tts_type: String,
    #[serde(default)]
    pub voice_elevenlabs_api_key: Option<String>,
    #[serde(default)]
    pub voice_elevenlabs_voice_id: Option<String>,
    #[serde(default)]
    pub use_tts: bool,
    #[serde(default)]
    pub voice_muted: bool,
    #[serde(default = "default_deafened")]
    pub voice_deafened: bool,
    #[serde(default)]
    pub voice_discord_guild_id: Option<String>,
    #[serde(default)]
    pub rvc_on: bool,
    #[serde(default)]
    pub rvc_server: Option<String>,
    #[serde(default)]
    pub rvc_model_path: Option<String>,
    #[serde(default)]
    pub rvc_index_path: Option<String>,
    #[serde(default)]
    pub voice_audio_output_path: Option<String>,
    #[serde(default)]
    pub qwen_tts_server: Option<String>,
    #[serde(default)]
    pub qwen_tts_model: Option<String>,
    #[serde(default)]
    pub qwen_tts_speaker: Option<String>,
    #[serde(default)]
    pub qwen_tts_language: Option<String>,
    #[serde(default)]
    pub minimax_api_key: Option<String>,
    #[serde(default)]
    pub minimax_voice_id: Option<String>,
    #[serde(default)]
    pub minimax_tts_model: Option<String>,
    #[serde(default)]
    pub mimo_api_key: Option<String>,
    #[serde(default)]
    pub mimo_voice_id: Option<String>,
    #[serde(default)]
    pub mimo_tts_type: Option<String>,
    #[serde(default)]
    pub history_with_toolcalls: bool,
    #[serde(default)]
    pub only_tool_calls_no_history: bool,
    #[serde(default)]
    pub download: bool,
    #[serde(default)]
    pub feedback_enabled: bool,
    #[serde(default = "default_feedback_max")]
    pub feedback_max_per_5min: i32,
    #[serde(default = "default_feedback_window")]
    pub feedback_window_secs: i32,
    #[serde(default = "default_allowed")]
    pub allowed_guilds: Vec<String>,
    #[serde(default = "default_allowed")]
    pub allowed_channels: Vec<String>,
    #[serde(default)]
    pub max_llm_turns: Option<i32>,
    #[serde(default)]
    pub max_tool_calls: Option<i32>,
    #[serde(default)]
    pub summarize_char_limit: Option<i32>,
}

fn default_stt() -> String { "vosk".to_string() }
fn default_tts() -> String { "windows_sapi".to_string() }
fn default_listen_timeout() -> i32 { 120 }
fn default_deafened() -> bool { true }
fn default_feedback_max() -> i32 { 10 }
fn default_feedback_window() -> i32 { 300 }
fn default_allowed() -> Vec<String> { vec!["*".to_string()] }

impl Default for ContextSettings {
    fn default() -> Self {
        Self {
            voice_enabled: false,
            voice_stt_type: default_stt(),
            voice_vosk_model_path: None,
            voice_whisper_model_path: None,
            voice_elevenlabs_stt_api_key: None,
            voice_last_input: None,
            voice_listen_timeout_secs: default_listen_timeout(),
            voice_owner_id: None,
            voice_tts_enabled: false,
            voice_tts_type: default_tts(),
            voice_elevenlabs_api_key: None,
            voice_elevenlabs_voice_id: None,
            use_tts: false,
            voice_muted: false,
            voice_deafened: default_deafened(),
            voice_discord_guild_id: None,
            rvc_on: false,
            rvc_server: None,
            rvc_model_path: None,
            rvc_index_path: None,
            voice_audio_output_path: None,
            qwen_tts_server: None,
            qwen_tts_model: None,
            qwen_tts_speaker: None,
            qwen_tts_language: None,
            minimax_api_key: None,
            minimax_voice_id: None,
            minimax_tts_model: None,
            mimo_api_key: None,
            mimo_voice_id: None,
            mimo_tts_type: None,
            history_with_toolcalls: false,
            only_tool_calls_no_history: false,
            download: false,
            feedback_enabled: false,
            feedback_max_per_5min: default_feedback_max(),
            feedback_window_secs: default_feedback_window(),
            allowed_guilds: default_allowed(),
            allowed_channels: default_allowed(),
            max_llm_turns: None,
            max_tool_calls: None,
            summarize_char_limit: None,
        }
    }
}

impl Database {
    pub fn load_context(&self, user_id: &str) -> anyhow::Result<Context> {
        let conn = self.conn();
        let result = conn.query_row(
            "SELECT data FROM contexts WHERE user_id = ?1",
            rusqlite::params![user_id],
            |row| row.get::<_, String>(0),
        );

        match result {
            Ok(data) => Ok(serde_json::from_str(&data)?),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(Context {
                user_id: user_id.to_string(),
                ..Default::default()
            }),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save_context(&self, ctx: &Context) -> anyhow::Result<()> {
        let conn = self.conn();
        let data = serde_json::to_string(ctx)?;
        conn.execute(
            "INSERT OR REPLACE INTO contexts (user_id, data, updated_at) VALUES (?1, ?2, datetime('now'))",
            rusqlite::params![ctx.user_id, data],
        )?;
        Ok(())
    }

    pub fn merge_context(&self, user_id: &str, updates: serde_json::Value) -> anyhow::Result<Context> {
        let mut ctx = self.load_context(user_id)?;
        let mut data: serde_json::Value = serde_json::to_value(&ctx)?;

        // Merge updates
        if let (Some(obj), Some(updates_obj)) = (data.as_object_mut(), updates.as_object()) {
            for (key, value) in updates_obj {
                obj.insert(key.clone(), value.clone());
            }
        }

        ctx = serde_json::from_value(data)?;
        self.save_context(&ctx)?;
        Ok(ctx)
    }

    pub fn increment_turn(&self, ctx: &mut Context) {
        ctx.turn += 1;
    }
}
```

## Messages

```rust
// src/db/messages.rs

use serde::{Deserialize, Serialize};
use super::Database;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Database {
    pub fn add_message(&self, user_id: &str, msg: &Message) -> anyhow::Result<i64> {
        let conn = self.conn();
        let id = conn.execute(
            "INSERT INTO messages (user_id, role, content, tool_call_id) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![user_id, msg.role, msg.content, msg.tool_call_id],
        )?;
        Ok(id as i64)
    }

    pub fn get_messages(&self, user_id: &str, limit: i32) -> anyhow::Result<Vec<Message>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT role, content, tool_call_id FROM messages WHERE user_id = ?1 ORDER BY id DESC LIMIT ?2"
        )?;

        let messages = stmt.query_map(rusqlite::params![user_id, limit], |row| {
            Ok(Message {
                role: row.get(0)?,
                content: row.get(1)?,
                tool_call_id: row.get(2)?,
            })
        })?.collect::<Result<Vec<_>, _>>()?;

        Ok(messages.into_iter().rev().collect())
    }

    pub fn clear_messages(&self, user_id: &str) -> anyhow::Result<()> {
        let conn = self.conn();
        conn.execute("DELETE FROM messages WHERE user_id = ?1", rusqlite::params![user_id])?;
        Ok(())
    }
}
```

## Secrets

```rust
// src/db/secrets.rs

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Secrets {
    pub discord_bot_token: Option<String>,
    pub minimax_api_key: Option<String>,
    pub mimo_api_key: Option<String>,
    pub openai_api_key: Option<String>,
    pub anthropic_api_key: Option<String>,
    pub elevenlabs_api_key: Option<String>,
    pub gateway_api_key: Option<String>,
}

static SECRETS: RwLock<Option<Secrets>> = RwLock::const_new(None);

pub async fn get_secrets() -> Option<Secrets> {
    SECRETS.read().await.clone()
}

pub async fn init_secrets(secrets: Secrets) {
    let mut lock = SECRETS.write().await;
    *lock = Some(secrets);
}

pub async fn update_secrets(new_secrets: Secrets) {
    let mut lock = SECRETS.write().await;
    *lock = Some(new_secrets);
}
```

---

# 6. Statemachine

Die Statemachine (.cl) ist eine State-Machine für Workflows. Sie definiert:
- States (Schritte im Workflow)
- Transitions (Wechsel zwischen Schritten)
- Auto-Rules (automatische Aktionen)
- Overrides (Context-Änderungen pro State)

```rust
// src/sm.rs

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmFile {
    pub name: String,
    pub initial_state: String,
    pub states: HashMap<String, SmState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmState {
    pub task_template: Option<String>,
    pub role_template: Option<String>,
    pub transitions: Vec<SmTransition>,
    pub auto_rules: Vec<SmAutoRule>,
    pub overrides: Option<serde_json::Value>,
    pub secret_overrides: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmTransition {
    pub target: String,
    pub condition: Option<String>,
    pub on: Option<String>, // tag name like "next", "done"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmAutoRule {
    pub condition: String,
    pub action: String, // "next", "set", "push", "pop"
    pub target: Option<String>,
    pub value: Option<String>,
}

impl SmFile {
    pub fn parse(content: &str) -> anyhow::Result<Self> {
        // Simple key-value parser for .cl files
        // Format:
        // [state_name]
        // task_template = tasks/understand
        // transition -> next_state on next
        // auto_rule: turn > 5 -> next

        let mut states = HashMap::new();
        let mut current_state: Option<String> = None;
        let mut current_state_data = SmStateBuilder::default();
        let mut initial_state = String::new();
        let mut name = String::new();

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            if line.starts_with('[') && line.ends_with(']') {
                // Save previous state
                if let Some(state_name) = current_state.take() {
                    if initial_state.is_empty() {
                        initial_state = state_name.clone();
                    }
                    states.insert(state_name, current_state_data.build());
                    current_state_data = SmStateBuilder::default();
                }
                current_state = Some(line[1..line.len()-1].to_string());
            } else if let Some(eq_pos) = line.find('=') {
                let key = line[..eq_pos].trim();
                let value = line[eq_pos+1..].trim();

                match key {
                    "name" => name = value.to_string(),
                    "task_template" => current_state_data.task_template = Some(value.to_string()),
                    "role_template" => current_state_data.role_template = Some(value.to_string()),
                    _ => {}
                }
            } else if line.contains("->") {
                // Transition: condition -> target on tag
                let parts: Vec<&str> = line.split("->").collect();
                if parts.len() == 2 {
                    let target = parts[1].trim().to_string();
                    current_state_data.transitions.push(SmTransition {
                        target,
                        condition: None,
                        on: Some(parts[0].trim().to_string()),
                    });
                }
            }
        }

        // Save last state
        if let Some(state_name) = current_state {
            if initial_state.is_empty() {
                initial_state = state_name.clone();
            }
            states.insert(state_name, current_state_data.build());
        }

        Ok(Self { name, initial_state, states })
    }
}

#[derive(Default)]
struct SmStateBuilder {
    task_template: Option<String>,
    role_template: Option<String>,
    transitions: Vec<SmTransition>,
    auto_rules: Vec<SmAutoRule>,
}

impl SmStateBuilder {
    fn build(self) -> SmState {
        SmState {
            task_template: self.task_template,
            role_template: self.role_template,
            transitions: self.transitions,
            auto_rules: self.auto_rules,
            overrides: None,
            secret_overrides: None,
        }
    }
}
```

## Example SM File

```
# contexts/default.cl
name = default

[understand]
task_template = tasks/understand
role_template = roles/senior_dev
next -> plan

[plan]
task_template = tasks/plan
next -> code

[code]
task_template = tasks/code
next -> review

[review]
task_template = tasks/review
next -> test

[test]
task_template = tasks/test
next -> done

[done]
task_template = tasks/done
```

---

# 7. Tags System

§-Tags sind spezielle Marker die der LLM in seiner Response verwenden kann um Aktionen auszulösen.

```rust
// src/tags.rs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Tag {
    Done,
    Next,
    Feedback { message: String },
    Push { path: String },
    Pop,
    Path { path: String },
    Mode { mode: String },
    Set { key: String, value: String },
    Learn { fact: String },
}

pub fn parse_tags(content: &str) -> (String, Vec<Tag>) {
    let mut tags = Vec::new();
    let mut clean_content = content.to_string();

    // §done
    if content.contains("§done") {
        tags.push(Tag::Done);
        clean_content = clean_content.replace("§done", "");
    }

    // §next
    if content.contains("§next") {
        tags.push(Tag::Next);
        clean_content = clean_content.replace("§next", "");
    }

    // §feedback{message}
    let feedback_re = regex::Regex::new(r"§feedback\{([^}]+)\}").unwrap();
    for cap in feedback_re.captures_iter(content) {
        tags.push(Tag::Feedback {
            message: cap[1].to_string(),
        });
        clean_content = clean_content.replace(&cap[0], "");
    }

    // §push{path}
    let push_re = regex::Regex::new(r"§push\{([^}]+)\}").unwrap();
    for cap in push_re.captures_iter(content) {
        tags.push(Tag::Push {
            path: cap[1].to_string(),
        });
        clean_content = clean_content.replace(&cap[0], "");
    }

    // §pop
    if content.contains("§pop") {
        tags.push(Tag::Pop);
        clean_content = clean_content.replace("§pop", "");
    }

    // §path{path}
    let path_re = regex::Regex::new(r"§path\{([^}]+)\}").unwrap();
    for cap in path_re.captures_iter(content) {
        tags.push(Tag::Path {
            path: cap[1].to_string(),
        });
        clean_content = clean_content.replace(&cap[0], "");
    }

    // §mode{mode}
    let mode_re = regex::Regex::new(r"§mode\{([^}]+)\}").unwrap();
    for cap in mode_re.captures_iter(content) {
        tags.push(Tag::Mode {
            mode: cap[1].to_string(),
        });
        clean_content = clean_content.replace(&cap[0], "");
    }

    // §set{key=value}
    let set_re = regex::Regex::new(r"§set\{([^=]+)=([^}]+)\}").unwrap();
    for cap in set_re.captures_iter(content) {
        tags.push(Tag::Set {
            key: cap[1].to_string(),
            value: cap[2].to_string(),
        });
        clean_content = clean_content.replace(&cap[0], "");
    }

    // §learn{fact}
    let learn_re = regex::Regex::new(r"§learn\{([^}]+)\}").unwrap();
    for cap in learn_re.captures_iter(content) {
        tags.push(Tag::Learn {
            fact: cap[1].to_string(),
        });
        clean_content = clean_content.replace(&cap[0], "");
    }

    // Clean up extra whitespace
    clean_content = regex::Regex::new(r"\n{3,}")
        .unwrap()
        .replace_all(&clean_content, "\n\n")
        .to_string();

    (clean_content.trim().to_string(), tags)
}
```

---

# 8. Event Channel

```rust
// src/event_channel.rs

use tokio::sync::broadcast;

#[derive(Debug, Clone)]
pub enum GatewayEvent {
    FileUpload {
        user_id: String,
        file_path: String,
        file_name: String,
    },
    AgentComplete {
        user_id: String,
        response: String,
    },
    AgentFeedback {
        user_id: String,
        message: String,
    },
    ChannelMessage {
        channel_id: String,
        content: String,
    },
    VoiceTts {
        guild_id: u64,
        audio_path: String,
    },
}

static EVENT_TX: once_cell::sync::OnceLock<broadcast::Sender<GatewayEvent>> =
    once_cell::sync::OnceLock::new();

pub fn init() -> broadcast::Receiver<GatewayEvent> {
    let (tx, rx) = broadcast::channel(100);
    EVENT_TX.set(tx).ok();
    rx
}

pub fn sender() -> broadcast::Sender<GatewayEvent> {
    EVENT_TX.get().unwrap().clone()
}

pub fn broadcast_event(event: GatewayEvent) {
    if let Some(tx) = EVENT_TX.get() {
        let _ = tx.send(event);
    }
}

pub fn broadcast_agent_complete(user_id: &str, response: &str) {
    broadcast_event(GatewayEvent::AgentComplete {
        user_id: user_id.to_string(),
        response: response.to_string(),
    });
}

pub fn broadcast_agent_feedback(user_id: &str, message: &str) {
    broadcast_event(GatewayEvent::AgentFeedback {
        user_id: user_id.to_string(),
        message: message.to_string(),
    });
}

pub fn broadcast_file_upload(user_id: &str, file_path: &str, file_name: &str) {
    broadcast_event(GatewayEvent::FileUpload {
        user_id: user_id.to_string(),
        file_path: file_path.to_string(),
        file_name: file_name.to_string(),
    });
}

pub fn broadcast_channel_message(channel_id: &str, content: &str) {
    broadcast_event(GatewayEvent::ChannelMessage {
        channel_id: channel_id.to_string(),
        content: content.to_string(),
    });
}

pub fn broadcast_voice_tts(guild_id: u64, audio_path: &str) {
    broadcast_event(GatewayEvent::VoiceTts {
        guild_id,
        audio_path: audio_path.to_string(),
    });
}
```

---

# 9. Gateway

## GatewayState

```rust
// src/gateway/mod.rs

use std::sync::Arc;
use tokio::sync::broadcast;

pub mod auth;
pub mod http_handler;
pub mod llm;
pub mod message_handler;
pub mod poml;
pub mod rate_limiter;
pub mod templates;
pub mod ws_handler;
pub mod cron_scheduler;

#[derive(Clone)]
pub struct GatewayState {
    pub db: crate::db::Database,
    pub config: crate::config::Config,
    pub secrets: crate::db::secrets::Secrets,
    pub llm: Arc<llm::LLMRouter>,
    pub event_tx: broadcast::Sender<crate::event_channel::GatewayEvent>,
    pub start_time: std::time::Instant,
}

pub async fn start(db: crate::db::Database, config: crate::config::Config) -> anyhow::Result<()> {
    let secrets = crate::db::secrets::get_secrets().await.unwrap_or_default();
    let event_tx = crate::event_channel::init();
    let llm = Arc::new(llm::LLMRouter::new(&config, &secrets));

    let state = GatewayState {
        db,
        config: config.clone(),
        secrets,
        llm,
        event_tx,
        start_time: std::time::Instant::now(),
    };

    let app = axum::Router::new()
        .route("/ws", axum::routing::get(ws_handler::ws_handler))
        .route("/health", axum::routing::get(http_handler::health_check))
        .route("/api/auth/login", axum::routing::post(auth::login_handler))
        .layer(axum::extract::Extension(state.clone()))
        .with_state(state);

    let addr = format!("0.0.0.0:{}", config.gateway_port);
    tracing::info!("Gateway listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
```

## Auth (NEU)

```rust
// src/gateway/auth.rs

use axum::{extract::Request, middleware::Next, response::Response, http::StatusCode, Json};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub exp: usize,
    pub iat: usize,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub password: String,
}

#[derive(Serialize)]
pub struct LoginResponse {
    pub token: String,
}

pub async fn login_handler(
    Json(payload): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, StatusCode> {
    let config = crate::config::Config::from_env();

    if !bcrypt::verify(&payload.password, &config.dashboard_admin_password).unwrap_or(false) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let claims = Claims {
        sub: "admin".to_string(),
        exp: (chrono::Utc::now() + chrono::Duration::hours(24)).timestamp() as usize,
        iat: chrono::Utc::now().timestamp() as usize,
    };

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(config.gateway_api_key.as_bytes()),
    ).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(LoginResponse { token }))
}

pub async fn auth_middleware(
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let config = crate::config::Config::from_env();

    let auth_header = req.headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let token = auth_header
        .strip_prefix("Bearer ")
        .ok_or(StatusCode::UNAUTHORIZED)?;

    // Try JWT first
    let jwt_result = decode::<Claims>(
        token,
        &DecodingKey::from_secret(config.gateway_api_key.as_bytes()),
        &Validation::default(),
    );

    if jwt_result.is_ok() {
        return Ok(next.run(req).await);
    }

    // Try API Key
    if token == config.gateway_api_key {
        return Ok(next.run(req).await);
    }

    Err(StatusCode::UNAUTHORIZED)
}
```

## Rate Limiter (NEU)

```rust
// src/gateway/rate_limiter.rs

use governor::{Quota, RateLimiter, state::keyed::DefaultKeyedStateStore};
use std::num::NonZeroU32;

pub struct UserRateLimiter {
    limiter: RateLimiter<String, DefaultKeyedStateStore<String>>,
}

impl UserRateLimiter {
    pub fn new(requests_per_minute: u32) -> Self {
        let quota = Quota::per_minute(NonZeroU32::new(requests_per_minute).unwrap());
        Self {
            limiter: RateLimiter::keyed(quota),
        }
    }

    pub fn check(&self, user_id: &str) -> Result<(), RateLimitError> {
        self.limiter.check_key(&user_id.to_string())
            .map_err(|_| RateLimitError::TooManyRequests)
    }
}

#[derive(Debug)]
pub enum RateLimitError {
    TooManyRequests,
}

impl std::fmt::Display for RateLimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Rate limit exceeded")
    }
}
```

## LLM Router (NEU)

```rust
// src/gateway/llm/mod.rs

pub mod provider;
pub mod openai;
pub mod anthropic;
pub mod ollama;
pub mod minimax;
pub mod mimo;

use provider::{LLMProvider, ChatRequest, ChatResponse};

pub struct LLMRouter {
    providers: Vec<Box<dyn LLMProvider>>,
    default_provider: String,
}

impl LLMRouter {
    pub fn new(config: &crate::config::Config, secrets: &crate::db::secrets::Secrets) -> Self {
        let mut providers: Vec<Box<dyn LLMProvider>> = Vec::new();

        // OpenAI
        if let Some(ref key) = secrets.openai_api_key {
            providers.push(Box::new(openai::OpenAIProvider::new(
                key.clone(),
                config.openai_model.clone(),
                config.openai_api_base.clone(),
            )));
        }

        // Anthropic
        if let Some(ref key) = secrets.anthropic_api_key {
            providers.push(Box::new(anthropic::AnthropicProvider::new(key.clone())));
        }

        // Ollama (always available, no key needed)
        providers.push(Box::new(ollama::OllamaProvider::new(
            config.ollama_api_base.clone(),
            config.ollama_model.clone(),
        )));

        // MiniMax
        if let Some(ref key) = secrets.minimax_api_key {
            providers.push(Box::new(minimax::MiniMaxProvider::new(key.clone())));
        }

        // MiMo
        if let Some(ref key) = secrets.mimo_api_key {
            providers.push(Box::new(mimo::MiMoProvider::new(key.clone())));
        }

        Self {
            providers,
            default_provider: config.use_provider.clone(),
        }
    }

    pub async fn chat(&self, request: ChatRequest, provider: Option<&str>) -> anyhow::Result<ChatResponse> {
        let provider_name = provider.unwrap_or(&self.default_provider);

        // Try specified provider
        for p in &self.providers {
            if p.name() == provider_name {
                match p.chat(request.clone()).await {
                    Ok(response) => return Ok(response),
                    Err(e) => {
                        tracing::warn!("Provider {} failed: {}", provider_name, e);
                        break;
                    }
                }
            }
        }

        // Fallback to any available provider
        for p in &self.providers {
            if p.name() != provider_name {
                match p.chat(request.clone()).await {
                    Ok(response) => {
                        tracing::info!("Fallback to {} successful", p.name());
                        return Ok(response);
                    }
                    Err(e) => {
                        tracing::warn!("Fallback {} failed: {}", p.name(), e);
                    }
                }
            }
        }

        Err(anyhow::anyhow!("All LLM providers failed"))
    }

    pub async fn health_check(&self) -> bool {
        for p in &self.providers {
            if p.health_check().await {
                return true;
            }
        }
        false
    }
}
```

## Provider Trait (NEU)

```rust
// src/gateway/llm/provider.rs

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub tools: Option<Vec<ToolDefinition>>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionDefinition,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub content: Option<String>,
    pub tool_calls: Option<Vec<ToolCall>>,
    pub finish_reason: Option<String>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[async_trait]
pub trait LLMProvider: Send + Sync {
    async fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse>;
    fn name(&self) -> &str;
    async fn health_check(&self) -> bool { true }
}
```

---

# 10. POML Renderer

```rust
// src/gateway/poml.rs

use std::process::Command;
use tempfile::NamedTempFile;

pub async fn render(template_path: &str, context: &serde_json::Value) -> anyhow::Result<String> {
    let poml_cli = std::env::var("POML_CLI")
        .unwrap_or_else(|_| "poml".to_string());

    // Write context to temp file
    let context_file = NamedTempFile::new()?;
    std::fs::write(context_file.path(), serde_json::to_string_pretty(context)?)?;

    // Try POML CLI
    let output = Command::new(&poml_cli)
        .arg("render")
        .arg(template_path)
        .arg("--context")
        .arg(context_file.path())
        .arg("--format")
        .arg("text")
        .output();

    match output {
        Ok(output) if output.status.success() => {
            let result = String::from_utf8(output.stdout)?;
            Ok(strip_think_tags(&result))
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!("POML CLI error: {}", stderr);
            // Fallback to simple template
            render_simple(template_path, context).await
        }
        Err(e) => {
            tracing::warn!("POML CLI not found: {}, using simple template", e);
            render_simple(template_path, context).await
        }
    }
}

async fn render_simple(template_path: &str, context: &serde_json::Value) -> anyhow::Result<String> {
    let template = std::fs::read_to_string(template_path)?;

    // Simple {{variable}} replacement
    let mut result = template.clone();
    if let Some(obj) = context.as_object() {
        for (key, value) in obj {
            let placeholder = format!("{{{{{}}}}}", key);
            let replacement = match value {
                serde_json::Value::String(s) => s.clone(),
                other => serde_json::to_string(other).unwrap_or_default(),
            };
            result = result.replace(&placeholder, &replacement);
        }
    }

    Ok(result)
}

pub fn strip_think_tags(content: &str) -> String {
    let re = regex::Regex::new(r"(?s)<think>.*?</think>").unwrap();
    re.replace_all(content, "").trim().to_string()
}

pub fn extract_agent_signals(content: &str) -> (String, Vec<String>) {
    let mut signals = Vec::new();
    let mut clean = content.to_string();

    // [[AGENT:NEXT]]
    if content.contains("[[AGENT:NEXT]]") {
        signals.push("next".to_string());
        clean = clean.replace("[[AGENT:NEXT]]", "");
    }

    // [[AGENT:COMPLETE]]
    if content.contains("[[AGENT:COMPLETE]]") {
        signals.push("complete".to_string());
        clean = clean.replace("[[AGENT:COMPLETE]]", "");
    }

    (clean.trim().to_string(), signals)
}
```

---

# 11-20: Details im Reference-Code

Die restlichen Module sind im Reference-Code dokumentiert. Implementiere NEU in `src/`, schlage im Reference-Code nach:

- **Discord**: `reference/src/discord/` (mod.rs, handler.rs, commands.rs, ws_client.rs)
- **Voice**: `reference/src/voice/mod.rs` (1365 Zeilen — komplexeste Komponente, direkt portieren)
- **Voice Handler**: `reference/src/voice/handler.rs` (249 Zeilen — Per-User Audio Buffers)
- **Tools**: `reference/src/tools/` (10 Dateien — execute_terminal, write_file, edit_file, etc.)
- **Dashboard**: `reference/src/dashboard/` (mod.rs, routes.rs)
- **DB**: `reference/src/db/` (10 Dateien — contexts, messages, memory, secrets, pairings, enc2)
- **POML**: `reference/src/gateway/poml.rs` (369 Zeilen — CLI Renderer)
- **Message Handler**: `reference/src/gateway/message_handler.rs` (1542 Zeilen — Agent Loop)
- **LLM**: `reference/src/gateway/llm.rs` (397 Zeilen — MiniMax/MiMo Client)
- **Templates**: `reference/templates/` (6 POML Dateien)
- **Migrations**: `reference/migrations/` (2 SQL Dateien)

---

# TDD Approach

## Phase 0: Security (Woche 1)

```
cargo test --lib -- security_tests       → FAIL (Tests schreiben)
... implementieren ...
cargo test --lib -- security_tests       → PASS
```

## Phase 1: DB (Woche 2)

```
cargo test --lib -- db_tests             → FAIL
... SQLite implementieren ...
cargo test --lib -- db_tests             → PASS
```

## Phase 2: Gateway (Woche 3)

```
cargo test --lib -- gateway_tests        → FAIL
... LLM Router, Auth, Streaming ...
cargo test --lib -- gateway_tests        → PASS
```

## Phase 3: Tools (Woche 4)

```
cargo test --lib -- tool_tests           → FAIL
... Tools implementieren ...
cargo test --lib -- tool_tests           → PASS
```

## Phase 4: Integration (Woche 5)

```
cargo test --test integration_tests      → FAIL
... Alles zusammen ...
cargo test --test integration_tests      → PASS
```
