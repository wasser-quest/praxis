//! Standalone terminal frontend: `praxis-tui`.
//!
//! This is the executable a TUI package installs. It reuses the same client as
//! `praxis chat`; the kernel (gateway/dashboard) is a separate process. A later
//! step moves this code into a `praxis-tui` crate so the kernel does not link it.
use clap::Parser;

#[derive(Parser)]
#[command(name = "praxis-tui")]
#[command(about = "Praxis terminal UI")]
struct Cli {
    /// Connect to a remote Praxis gateway (e.g. http://host:3537)
    #[arg(long)]
    gateway_url: Option<String>,
    /// Gateway API key (or set PRAXIS_GATEWAY_KEY to avoid shell history)
    #[arg(long)]
    gateway_key: Option<String>,
}

fn init_logging() {
    // The TUI owns the terminal, so logs go to a file only.
    let log_dir = std::env::var("LOG_DIR").unwrap_or_else(|_| "./logs".to_string());
    let _ = std::fs::create_dir_all(&log_dir);
    let file_appender = tracing_appender::rolling::daily(&log_dir, "praxis-tui.log");
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    use tracing_subscriber::prelude::*;
    let _ = tracing_subscriber::registry()
        .with(env_filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(file_writer)
                .with_ansi(false),
        )
        .try_init();
    // Leak the guard so logs flush for the process lifetime.
    std::mem::forget(guard);
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    init_logging();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(praxis::tui::run_chat(cli.gateway_url, cli.gateway_key))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_gateway_flags() {
        let cli = Cli::try_parse_from([
            "praxis-tui",
            "--gateway-url",
            "http://127.0.0.1:3537",
            "--gateway-key",
            "secret",
        ])
        .unwrap();
        assert_eq!(cli.gateway_url.as_deref(), Some("http://127.0.0.1:3537"));
        assert_eq!(cli.gateway_key.as_deref(), Some("secret"));
    }
}
