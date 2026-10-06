//! Terminal-UI chat client for Praxis.
//!
//! Architecture:
//!
//! ```text
//!     ┌──────────────┐   HTTP /v1   ┌────────────────────┐
//!     │  praxis-tui  │  ──────────▶ │  gateway (3537)    │
//!     │  (this crate)│  ◀────────── │  chat, history,    │
//!     └──────────────┘   SSE + JSON │  context, sessions │
//!                                    └────────────────────┘
//! ```
//!
//! - History, sessions and context edits are backend-authoritative: the
//!   terminal UI never opens the local database and does not link the kernel.
//! - Sessions follow the same model as the web chat: each session is
//!   its own `user_id` at the storage layer; new sessions are forked
//!   from the parent context so they inherit settings but start with
//!   an empty message log.
//! - The agent loop is invoked over HTTP to the local gateway. The
//!   TUI subscribes automatically to authenticated gateway SSE for text,
//!   reasoning and tool-call fragments. SQLite polling reconciles durable
//!   results and provides history/reconnect recovery.

mod app;
mod model;
mod remote;
mod sessions;
mod sse;
mod streaming;
mod tool_result;
mod ui;

pub use app::run as run_chat;
