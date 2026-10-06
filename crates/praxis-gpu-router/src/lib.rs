//! GPU-Router-Integration (pgpu, Bauplan §12.4).
//!
//! Praxis erreicht die GPU-Boxen über den pgpu-Router (gleiche Ports wie
//! direkt: 8188/2700/11434-11436). Der Router antwortet auf kaltem Slot mit
//! `503 + Retry-After + X-Router-State`; mit `X-Router-Wait: <s>` hält er
//! die Verbindung bis healthy (impliziter Wake). Dieses Modul liefert:
//! - `state()` (5-s-Cache) für Badges/Entscheidungen,
//! - `wake()`/`ensure_awake()` (dedupliziert, fire-and-forget),
//! - `wait_s()` für den `X-Router-Wait`-Header des llamacpp-Adapters,
//! - `job_id()` für `X-Router-Job-Id` (ComfyUI-Batches; Slot bleibt busy,
//!   kein Idle-Stop zwischen den Sätzen eines Antwortblocks).
//!
//! Konfiguration (Env, wie LLAMACPP_API_BASE):
//! - `GPU_ROUTER_URL`     Basis-URL des Router-Dashboards, z. B. http://100.105.6.69:8080
//! - `GPU_ROUTER_TOKEN`   Bearer-Token (identisch mit ROUTER_TOKEN im Router-Deploy)
//! - `GPU_ROUTER_WAIT_S`  Hold-Fenster für X-Router-Wait (Default 120)
//! Ohne `GPU_ROUTER_URL` sind alle Funktionen sichere No-Ops — der Router ist
//! optional, lokale Direkt-Setups bleiben unberührt.

use once_cell::sync::Lazy;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Slot-Rollen-Nummern des Routers (pgpu-Konvention).
pub const SLOT_LLM: i64 = 1;
pub const SLOT_MEDIA: i64 = 2;

pub const HEADER_WAIT: &str = "x-router-wait";
pub const HEADER_JOB_ID: &str = "x-router-job-id";
pub const HEADER_PRIORITY: &str = "x-router-priority";

const STATE_CACHE_S: Duration = Duration::from_secs(5);
const WAKE_DEDUPE_S: Duration = Duration::from_secs(30);
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct RouterConfig {
    pub url: String,
    pub token: String,
    pub wait_s: u64,
}

impl RouterConfig {
    fn from_env() -> Option<Self> {
        let url = std::env::var("GPU_ROUTER_URL").ok()?.trim().to_string();
        if url.is_empty() {
            return None;
        }
        let wait_s = std::env::var("GPU_ROUTER_WAIT_S")
            .ok()
            .and_then(|v| v.trim().parse().ok());
        let token = std::env::var("GPU_ROUTER_TOKEN").unwrap_or_default();
        Some(Self::from_parts(url, wait_s, token))
    }

    /// Reine Konstruktionslogik (testbar ohne Env-Racen zwischen Tests):
    /// URL-Normalisierung + Wait-Kappung auf das Router-Maximum.
    fn from_parts(url: String, wait_s: Option<u64>, token: String) -> Self {
        Self {
            url: url.trim_end_matches('/').to_string(),
            token,
            wait_s: wait_s.unwrap_or(120).min(600),
        }
    }
}

static CONFIG: Lazy<Option<RouterConfig>> = Lazy::new(RouterConfig::from_env);

pub fn configured() -> bool {
    CONFIG.is_some()
}

/// Hold-Fenster für `X-Router-Wait` (Default 120 s — passt in die 180-s-
/// Attempt-Deadline und deckt den ~1–2-min-Neustart einer gestoppten Box).
pub fn wait_s() -> u64 {
    CONFIG.as_ref().map_or(0, |c| c.wait_s)
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SlotState {
    pub id: i64,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub healthy: bool,
    #[serde(default)]
    pub busy: bool,
    #[serde(default)]
    pub desired_running: bool,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub active_instance: Option<i64>,
}

/// Budget-Teil des Router-States (fürs Dashboard-Badge: „heute X $“).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RouterBudget {
    #[serde(default)]
    pub spent_today_usd: f64,
    #[serde(default)]
    pub spent_month_usd: f64,
    #[serde(default)]
    pub soft_eur: f64,
    #[serde(default)]
    pub hard_eur: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RouterState {
    #[serde(default)]
    pub slots: Vec<SlotState>,
    #[serde(default)]
    pub stt_sessions: u64,
    #[serde(default)]
    pub budget: Option<RouterBudget>,
    /// Setzt das Dashboard-Proxy-Endpoint, wenn kein Router konfiguriert ist.
    #[serde(default)]
    pub configured: Option<bool>,
}

static STATE_CACHE: Lazy<Mutex<Option<(Instant, RouterState)>>> = Lazy::new(|| Mutex::new(None));
static LAST_WAKE: Lazy<Mutex<HashMap<i64, Instant>>> = Lazy::new(|| Mutex::new(HashMap::new()));

fn http_client() -> Client {
    Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .expect("static GPU router HTTP client")
}

/// `GET /api/v1/state` mit 5-s-Cache. None: Router nicht konfiguriert oder
/// (vorübergehend) nicht erreichbar — Aufrufer behandeln das als "unbekannt".
pub async fn state() -> Option<RouterState> {
    let cfg = CONFIG.as_ref()?;
    {
        let cache = STATE_CACHE.lock().unwrap();
        if let Some((at, snapshot)) = cache.as_ref() {
            if at.elapsed() < STATE_CACHE_S {
                return Some(snapshot.clone());
            }
        }
    }
    let mut request = http_client().get(format!("{}/api/v1/state", cfg.url));
    if !cfg.token.is_empty() {
        request = request.bearer_auth(&cfg.token);
    }
    match request.send().await {
        Ok(response) if response.status().is_success() => {
            match response.json::<RouterState>().await {
                Ok(snapshot) => {
                    *STATE_CACHE.lock().unwrap() = Some((Instant::now(), snapshot.clone()));
                    Some(snapshot)
                }
                Err(error) => {
                    tracing::debug!(%error, "GPU-Router: State-Antwort unlesbar");
                    None
                }
            }
        }
        Ok(response) => {
            tracing::debug!(status = %response.status(), "GPU-Router: State-Endpunkt nicht ok");
            None
        }
        Err(error) => {
            tracing::debug!(%error, "GPU-Router nicht erreichbar (State)");
            None
        }
    }
}

/// Slot-Vorschau aus dem State-Cache.
pub async fn slot(slot_id: i64) -> Option<SlotState> {
    state().await?.slots.into_iter().find(|s| s.id == slot_id)
}

/// `POST /api/v1/slots/{id}/wake` — fire-and-forget, dedupliziert (30 s/Slot).
/// Idempotent: der Router ignoriert Wake auf bereits laufenden Slots.
pub async fn wake(slot_id: i64) -> bool {
    let Some(cfg) = CONFIG.as_ref() else {
        return false;
    };
    {
        let mut last = LAST_WAKE.lock().unwrap();
        if last.get(&slot_id).is_some_and(|at| at.elapsed() < WAKE_DEDUPE_S) {
            return true; // bereits kürzlich angestoßen
        }
        last.insert(slot_id, Instant::now());
    }
    let mut request = http_client().post(format!("{}/api/v1/slots/{slot_id}/wake", cfg.url));
    if !cfg.token.is_empty() {
        request = request.bearer_auth(&cfg.token);
    }
    // Force: auch bei ausgeschalteter Auto-Miete (Nacht-Sleep) wecken —
    // Praxis ist der authentifizierte Auslöser, Budget-Caps greifen weiter.
    request = request.json(&serde_json::json!({ "force": true }));
    match request.send().await {
        Ok(response) if response.status().is_success() => {
            tracing::info!(slot = slot_id, "GPU-Router: Slot geweckt");
            true
        }
        Ok(response) => {
            tracing::debug!(slot = slot_id, status = %response.status(), "GPU-Router: wake abgelehnt");
            false
        }
        Err(error) => {
            tracing::debug!(slot = slot_id, %error, "GPU-Router nicht erreichbar (wake)");
            false
        }
    }
}

/// Wake nur, wenn der Slot wirklich kalt ist (nicht healthy, nicht bereits
/// am Wärmen mit desired_running). Läuft typischerweise im Hintergrund:
/// Chat-Turn startet → LLM warmt durch X-Router-Wait, media wird preemptiv
/// geweckt, während die Antwort noch generiert.
pub async fn ensure_awake(slot_id: i64) -> bool {
    if let Some(slot) = slot(slot_id).await {
        if slot.healthy {
            return true;
        }
        // Wärmt der Router bereits (desired_running gesetzt, z. B. impliziter
        // Wake durch X-Router-Wait oder ein laufender Replace), nicht erneut
        // anstoßen — der Router behandelt Wake zwar idempotent, aber das
        // Dedupe-Log hier vermeidet Rauschen im Event-Log.
        if slot.desired_running {
            return true;
        }
    }
    // State unbekannt (Router nicht konfiguriert/erreichbar): wake() no-ops
    // ohne Konfiguration; sonst einmal anstoßen — besser ein Idempotent-Wake
    // als ein stummer Kaltlauf.
    wake(slot_id).await
}

/// Nicht-blockierender `ensure_awake` für Request-Pfade (Chat-Turn, TTS).
pub fn ensure_awake_background(slot_id: i64) {
    if !configured() {
        return;
    }
    tokio::spawn(async move {
        let _ = ensure_awake(slot_id).await;
    });
}

/// Reaktion auf `503 + X-Router-State` vom ComfyUI-Proxy: Slot anstoßen,
/// damit der NÄCHSTE Versuch (nächste Antwort, nächster Satz) eine warme
/// Box vorfindet. Der laufende Versuch scheitert bewusst schnell — Fallback
/// statt Minuten-Warten auf einen 20-min-Warmup.
pub fn on_cold_response(slot_id: i64, state: &str) {
    tracing::warn!(
        slot = slot_id,
        router_state = %state,
        "GPU-Slot kalt/warmend — Wake angestoßen, dieser Versuch nutzt den Fallback"
    );
    ensure_awake_background(slot_id);
}

/// Batch-Korrelations-ID für `X-Router-Job-Id` (TTS-Sätze eines Antwort-
/// blocks = ein Job ⇒ Router hält den Slot über die Lücken busy).
pub fn job_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Session-Start-Hook (pgpu-Bauplan §12.4 🟢): Beim Öffnen einer Chat-
/// Session (Dashboard-WS) oder Discord-Voice-Join beide Slots anstoßen.
/// Fire-and-forget + dedupliziert (30 s/Slot) — ein Kaltstart des LLM-Slots
/// dauert bis 45 min; der Wake soll schon beim Öffnen laufen, nicht erst
/// beim ersten Chat-Turn (der sonst 5× ins Retry läuft).
pub fn wake_slots_for_session() {
    ensure_awake_background(SLOT_LLM);
    ensure_awake_background(SLOT_MEDIA);
}

/// Der ComfyUI-Proxy des Routers antwortet auf kaltem Slot mit 503 +
/// `X-Router-State`. Typisierter Marker, damit Aufrufer (ComfyUiClient)
/// gezielt wake anstoßen und den Versuch schnell per Fallback beenden
/// können, statt die Meldung als String zu raten.
#[derive(Debug)]
pub struct RouterCold {
    pub state: String,
}

impl std::fmt::Display for RouterCold {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GPU-Slot {}", self.state)
    }
}
impl std::error::Error for RouterCold {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn router_config_normalizes_and_clamps() {
        let cfg = RouterConfig::from_parts("http://100.105.6.69:8080/".into(), Some(9999), String::new());
        assert_eq!(cfg.url, "http://100.105.6.69:8080", "trailing slash wird entfernt");
        assert_eq!(cfg.wait_s, 600, "wait_s wird auf das Router-Maximum gekappt");
        assert!(cfg.token.is_empty());

        let default = RouterConfig::from_parts("http://r:8080".into(), None, "tok".into());
        assert_eq!(default.wait_s, 120, "Default-Hold 120 s passt in die 180-s-Attempt-Deadline");
        assert_eq!(default.token, "tok");
    }

    #[test]
    fn job_ids_are_unique() {
        assert_ne!(job_id(), job_id());
        assert!(uuid::Uuid::parse_str(&job_id()).is_ok());
    }

    #[test]
    fn state_json_matches_router_schema() {
        // Live-Schema des Routers (GET /api/v1/state) gegen RouterState prüfen:
        // Felder, die Praxis liest, müssen deserialisierbar bleiben — der
        // Router ergänzt Felder nur additive (serde-default hier unten).
        let body = serde_json::json!({
            "budget": {"date": "2026-09-20", "soft_eur": 2.0, "hard_eur": 2.4,
                       "monthly_eur": 50.0, "spent_today_usd": 1.5,
                       "spent_month_usd": 3.0, "usd_per_eur": 1.08},
            "slots": [
                {"id": 1, "role": "llm", "name": "LLM", "healthy": true, "busy": false,
                 "desired_running": true, "state": "healthy", "active_instance": 123,
                 "in_flight": 0, "progress": null},
                {"id": 2, "role": "media", "healthy": false, "busy": false,
                 "desired_running": false, "state": null}
            ],
            "stt_sessions": 2
        });
        let parsed: RouterState = serde_json::from_value(body).expect("schema match");
        assert_eq!(parsed.slots.len(), 2);
        assert!(parsed.slots[0].healthy);
        assert_eq!(parsed.slots[0].active_instance, Some(123));
        assert_eq!(parsed.slots[1].state, None);
        assert_eq!(parsed.stt_sessions, 2);
    }

    #[test]
    fn state_json_tolerates_missing_optional_fields() {
        // Minimale Antwort: Slots mit nur id — alles andere defaulted.
        let body = serde_json::json!({ "slots": [ { "id": 2 } ] });
        let parsed: RouterState = serde_json::from_value(body).expect("minimal schema match");
        assert_eq!(parsed.slots.len(), 1);
        assert!(!parsed.slots[0].healthy);
        assert!(!parsed.slots[0].desired_running);
        assert_eq!(parsed.stt_sessions, 0);
    }
}
