//! Replaceable runtime engine seam.
//!
//! The kernel always observes evidence (exit codes, timeouts, resource bytes)
//! and issues receipts. An engine owns *interpretation policy*: how templates
//! render and how skills/instructions are produced. The default `KernelEngine`
//! wraps the built-in renderer, so behavior is unchanged. A `runtime`-role
//! package may install another engine through the host bridge; guards continue
//! to compare against kernel-observed evidence.
use async_trait::async_trait;
use serde_json::Value;
use std::{
    path::Path,
    sync::{Arc, RwLock},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineInfo {
    pub id: String,
    pub version: String,
}

#[async_trait]
pub trait RuntimeEngine: Send + Sync {
    fn info(&self) -> EngineInfo;
    /// Lenient render with the built-in fallback path.
    async fn render(&self, template_path: &str, context: &Value) -> anyhow::Result<String>;
    /// Strict render: no fallback, unknown variables fail.
    async fn render_strict(&self, template_path: &str, context: &Value) -> anyhow::Result<String>;
    /// Validate a prospective template before publishing it.
    async fn render_strict_candidate(
        &self,
        template_path: &str,
        context: &Value,
        destination: Option<&Path>,
    ) -> anyhow::Result<String>;
    /// Evaluate a state-machine guard/transition condition. The kernel owns the
    /// parsed state machine and the evidence; the engine owns the meaning of a
    /// condition.
    fn evaluate_condition(
        &self,
        condition: &str,
        context: &serde_json::Map<String, Value>,
    ) -> bool;
}

/// The built-in engine: today's POML renderer, unchanged.
pub struct KernelEngine;

#[async_trait]
impl RuntimeEngine for KernelEngine {
    fn info(&self) -> EngineInfo {
        EngineInfo {
            id: "kernel".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        }
    }

    async fn render(&self, template_path: &str, context: &Value) -> anyhow::Result<String> {
        crate::gateway::poml::render(template_path, context).await
    }

    async fn render_strict(&self, template_path: &str, context: &Value) -> anyhow::Result<String> {
        crate::gateway::poml::render_strict(template_path, context).await
    }

    async fn render_strict_candidate(
        &self,
        template_path: &str,
        context: &Value,
        destination: Option<&Path>,
    ) -> anyhow::Result<String> {
        crate::gateway::poml::render_strict_candidate(template_path, context, destination).await
    }

    fn evaluate_condition(
        &self,
        condition: &str,
        context: &serde_json::Map<String, Value>,
    ) -> bool {
        crate::sm::evaluate_condition(condition, context)
    }
}

static INSTALLED: RwLock<Option<Arc<dyn RuntimeEngine>>> = RwLock::new(None);

/// Install a replacement engine. Callers must already hold a `runtime` grant
/// from the operator trust store; this Rust API does not check that itself.
pub fn install(engine: Arc<dyn RuntimeEngine>) {
    if let Ok(mut slot) = INSTALLED.write() {
        *slot = Some(engine);
    }
}

/// Return to the built-in engine (used on shutdown and by tests).
pub fn clear() {
    if let Ok(mut slot) = INSTALLED.write() {
        *slot = None;
    }
}

/// The active engine, or `KernelEngine`.
pub fn current() -> Arc<dyn RuntimeEngine> {
    INSTALLED
        .read()
        .ok()
        .and_then(|slot| slot.clone())
        .unwrap_or_else(|| Arc::new(KernelEngine))
}

pub fn info() -> EngineInfo {
    current().info()
}

pub fn evaluate_condition(
    condition: &str,
    context: &serde_json::Map<String, Value>,
) -> bool {
    current().evaluate_condition(condition, context)
}

pub async fn render(template_path: &str, context: &Value) -> anyhow::Result<String> {
    let engine = current();
    engine.render(template_path, context).await
}

pub async fn render_strict(template_path: &str, context: &Value) -> anyhow::Result<String> {
    let engine = current();
    engine.render_strict(template_path, context).await
}

pub async fn render_strict_candidate(
    template_path: &str,
    context: &Value,
    destination: Option<&Path>,
) -> anyhow::Result<String> {
    let engine = current();
    engine
        .render_strict_candidate(template_path, context, destination)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct CountingEngine {
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl RuntimeEngine for CountingEngine {
        fn info(&self) -> EngineInfo {
            EngineInfo {
                id: "fixture".into(),
                version: "0".into(),
            }
        }
        async fn render(&self, template_path: &str, context: &Value) -> anyhow::Result<String> {
            KernelEngine.render(template_path, context).await
        }
        async fn render_strict(
            &self,
            template_path: &str,
            context: &Value,
        ) -> anyhow::Result<String> {
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            KernelEngine.render_strict(template_path, context).await
        }
        async fn render_strict_candidate(
            &self,
            template_path: &str,
            context: &Value,
            destination: Option<&Path>,
        ) -> anyhow::Result<String> {
            KernelEngine
                .render_strict_candidate(template_path, context, destination)
                .await
        }
        fn evaluate_condition(
            &self,
            condition: &str,
            context: &serde_json::Map<String, Value>,
        ) -> bool {
            // Override one marker condition and delegate everything else so
            // concurrent tests see unchanged behavior while the fixture is set.
            condition == "axiom" || KernelEngine.evaluate_condition(condition, context)
        }
    }

    #[tokio::test]
    async fn engine_default_override_and_reset() {
        clear();
        assert_eq!(info().id, "kernel");
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        install(Arc::new(CountingEngine { calls: calls.clone() }));
        assert_eq!(info().id, "fixture");
        // A missing template is still resolved by the kernel engine; the call
        // counter proves the installed engine handled the request.
        let _ = render_strict("does-not-exist.poml", &json!({})).await;
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        // Guard-condition policy also comes from the engine.
        let empty = serde_json::Map::new();
        assert!(evaluate_condition("axiom", &empty));
        // Other conditions still delegate to the kernel implementation.
        assert!(!evaluate_condition("axiom == false", &empty));
        clear();
        assert_eq!(info().id, "kernel");
        assert!(!evaluate_condition("axiom", &empty));
    }
}
