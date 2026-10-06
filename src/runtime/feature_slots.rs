//! Registered feature navigation/page slots.
//!
//! A slot names a feature that can contribute a dashboard page. It exists so a
//! client can explain how to obtain a page it cannot show right now, instead of
//! silently hiding the feature. A registration is only a title and a hint: it
//! grants no authority and, crucially, listing slots never starts, initializes
//! or contacts a feature service just to draw UI.
use crate::plugins::PluginRegistry;
use serde::Serialize;
use std::collections::BTreeMap;

/// A first-party feature package that ships (or can be installed) with its own
/// page. Slots are reported whether or not a live contribution backs them.
pub struct FeatureSlot {
    pub id: &'static str,
    pub title: &'static str,
    pub hint: &'static str,
}

pub const FEATURE_SLOTS: &[FeatureSlot] = &[FeatureSlot {
    id: "vm",
    title: "Virtual Machines",
    hint: "Install and enable the VM package (see docs/INSTALLATION_PRESETS.md) to administer guests here.",
}];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SlotStatus {
    pub id: String,
    pub title: String,
    /// Whether a live web contribution currently backs the slot.
    pub present: bool,
    /// How to make the feature available when it is not present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl SlotStatus {
    fn absent(id: impl Into<String>, title: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            present: false,
            hint: Some(hint.into()),
        }
    }
    fn present(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            present: true,
            hint: None,
        }
    }
}

/// Every registered slot, plus any installed package contribution that is not
/// currently live. An unready or disabled binding is reported as absent with an
/// enable/repair hint; `web_endpoint` is consulted read-only, so nothing is
/// initialized here.
pub fn feature_slots(plugins: &PluginRegistry) -> Vec<SlotStatus> {
    let mut slots: BTreeMap<String, SlotStatus> = BTreeMap::new();
    for slot in FEATURE_SLOTS {
        slots.insert(
            slot.id.to_string(),
            SlotStatus::absent(slot.id, slot.title, slot.hint),
        );
    }
    // Installed package contributions are registered even when their service is
    // not ready, so an operator can see what is missing and why.
    for plugin in plugins.list().into_iter().filter(|plugin| plugin.enabled) {
        let Some(web) = &plugin.provides.web else {
            continue;
        };
        slots.entry(plugin.name.clone()).or_insert_with(|| {
            SlotStatus::absent(
                plugin.name.clone(),
                web.title.clone(),
                format!(
                    "The '{}' service is not ready. Enable package '{}' and restart Praxis.",
                    web.service, plugin.name
                ),
            )
        });
    }
    // A live binding marks its slot present (and registers it when it is not
    // otherwise known). Availability only; this reads state, never starts work.
    for service in plugins.web_services() {
        if service.web_endpoint().is_none() {
            continue;
        }
        let owner = service.descriptor().owner.clone();
        let title = service
            .descriptor()
            .web
            .as_ref()
            .map(|web| web.title.clone())
            .unwrap_or_else(|| owner.clone());
        slots
            .entry(owner.clone())
            .and_modify(|slot| {
                slot.present = true;
                slot.hint = None;
            })
            .or_insert_with(|| SlotStatus::present(owner, title));
    }
    slots.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::Plugin;
    use crate::runtime::features::{InvocationContext, NativeService};
    use crate::runtime::web::WebEndpoint;
    use praxis_plugin_api::web::{WebDescriptor, WebInfo, WEB_API_VERSION};
    use serde_json::Value;

    /// A service whose contribution is live: it reports a validated descriptor
    /// and a reachable loopback endpoint.
    struct Up;
    #[async_trait::async_trait]
    impl NativeService for Up {
        fn web_descriptor(&self) -> Option<WebDescriptor> {
            Some(WebDescriptor::for_package("ext", "Extension"))
        }
        fn web_endpoint(&self) -> Option<WebEndpoint> {
            Some(WebEndpoint::new(
                WebInfo {
                    version: WEB_API_VERSION,
                    port: 9,
                    descriptor: WebDescriptor::for_package("ext", "Extension"),
                },
                "host-issued-key".into(),
            ))
        }
        async fn invoke(
            &self,
            _: InvocationContext,
            _: &str,
            _: Value,
        ) -> anyhow::Result<Value> {
            unreachable!("slot listing must not invoke a service")
        }
    }

    /// A registered service with no web contribution of its own.
    struct Down;
    #[async_trait::async_trait]
    impl NativeService for Down {
        async fn invoke(
            &self,
            _: InvocationContext,
            _: &str,
            _: Value,
        ) -> anyhow::Result<Value> {
            unreachable!("slot listing must not invoke a service")
        }
    }

    fn plugin(name: &str, service: &str, title: &str) -> Plugin {
        serde_json::from_value(serde_json::json!({
            "name": name, "description": "x", "version": "1",
            "provides": {
                "routes": [name],
                "web": {"service": service, "title": title}
            },
            "tools": [{
                "name": format!("{name}_echo"),
                "description": "x",
                "parameters": {"type": "object"},
                "handler": {
                    "type": "service", "service": service, "operation": "echo",
                    "api_version": 1, "timeout_secs": 5, "executable": "worker"
                }
            }]
        }))
        .unwrap()
    }

    #[test]
    fn registered_slots_are_absent_without_a_live_contribution() {
        let slots = feature_slots(&PluginRegistry::new());
        let vm = slots.iter().find(|slot| slot.id == "vm").expect("vm slot");
        assert!(!vm.present);
        assert!(vm.hint.as_deref().unwrap_or_default().contains("Install"));
    }

    #[test]
    fn installed_but_unready_contribution_is_reported_as_absent_with_a_hint() {
        let mut plugins = PluginRegistry::new();
        plugins
            .try_register(plugin("ext", "ext", "Extension"))
            .unwrap();
        let slots = feature_slots(&plugins);
        let ext = slots.iter().find(|slot| slot.id == "ext").expect("ext slot");
        assert!(!ext.present);
        assert!(ext.hint.as_deref().unwrap_or_default().contains("'ext'"));
    }

    #[test]
    fn a_live_binding_marks_its_slot_present_without_a_hint() {
        let mut plugins = PluginRegistry::new();
        plugins
            .try_register(plugin("ext", "ext", "Extension"))
            .unwrap();
        plugins
            .register_service("ext", "ext", 1, &["echo"], std::sync::Arc::new(Up))
            .unwrap();
        let slots = feature_slots(&plugins);
        let ext = slots.iter().find(|slot| slot.id == "ext").expect("ext slot");
        assert!(ext.present);
        assert!(ext.hint.is_none());
    }

    #[test]
    fn slot_listing_never_initializes_a_binding() {
        // `Down` has no web endpoint and panics on invoke: a service without a
        // live contribution is reported as absent rather than being started.
        let mut plugins = PluginRegistry::new();
        plugins
            .try_register(plugin("ext", "ext", "Extension"))
            .unwrap();
        plugins
            .register_service("ext", "ext", 1, &["echo"], std::sync::Arc::new(Down))
            .unwrap();
        let slots = feature_slots(&plugins);
        assert!(slots.iter().any(|slot| slot.id == "ext" && !slot.present));
    }
}
