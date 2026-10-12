//! Reactive application state for the GritIRT console.
//!
//! One [`AppState`] value holds every signal the console reads or writes,
//! provided once via `provide!` in `run_app` and pulled out of context with
//! `use_context::<AppState>()`.
//!
//! Live updates arrive over SSE (`incident` | `alert` | `timeline` | `run` |
//! `step` named events, see [`crate::net`]). The incident queue is patched
//! in place; the selected incident's dossier (alerts, timeline, runs) is
//! re-fetched on relevant frames.

use std::cell::Cell;
use std::rc::Rc;

use serde::{Deserialize, Serialize};
use veloasm::prelude::*;

// ---------------------------------------------------------------------------
// Buffer sizing
// ---------------------------------------------------------------------------

/// Incidents retained client-side (server is the source of truth; the list
/// endpoint re-syncs on demand).
pub const INCIDENT_KEEP: usize = 500;
/// Flash alerts visible at once.
pub const TOAST_KEEP: usize = 5;
/// Milliseconds before a flash alert auto-dismisses.
pub const TOAST_TTL_MS: u32 = 8_000;

// ---------------------------------------------------------------------------
// Display enums
// ---------------------------------------------------------------------------

/// Backend statuses: open | triaging | investigating | contained |
/// eradicating | recovering | resolved | closed.
pub const STATUSES: [&str; 8] = [
    "open",
    "triaging",
    "investigating",
    "contained",
    "eradicating",
    "recovering",
    "resolved",
    "closed",
];

/// Terminal states stop the live treatment (no auto re-fetch pressure).
pub fn is_terminal_status(s: &str) -> bool {
    matches!(s, "resolved" | "closed")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    /// Unknown inputs normalise to `medium` (matches the backend).
    pub fn parse(s: &str) -> Severity {
        match s.trim().to_ascii_lowercase().as_str() {
            "low" => Severity::Low,
            "high" => Severity::High,
            "critical" => Severity::Critical,
            _ => Severity::Medium,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Severity::Low => "LOW",
            Severity::Medium => "MEDIUM",
            Severity::High => "HIGH",
            Severity::Critical => "CRITICAL",
        }
    }

    /// Full Tailwind badge classes as literals (kept whole so the Tailwind
    /// scanner sees them; never build these with `format!`).
    pub fn badge(&self) -> &'static str {
        match self {
            Severity::Low => "bg-sky-500/15 text-sky-300",
            Severity::Medium => "bg-slate-500/15 text-slate-300",
            Severity::High => "bg-amber-500/15 text-amber-300",
            Severity::Critical => "bg-rose-500/15 text-rose-300 blink",
        }
    }

    /// Left-border accent for queue cards (full literals, see [`Self::badge`]).
    pub fn edge(&self) -> &'static str {
        match self {
            Severity::Low => "border-l-sky-400",
            Severity::Medium => "border-l-slate-500",
            Severity::High => "border-l-amber-400",
            Severity::Critical => "border-l-rose-500",
        }
    }

    pub fn rank(&self) -> u8 {
        match self {
            Severity::Low => 0,
            Severity::Medium => 1,
            Severity::High => 2,
            Severity::Critical => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnStatus {
    Connecting,
    Open,
    Reconnecting,
    Closed,
}

impl ConnStatus {
    pub fn label(&self) -> &'static str {
        match self {
            ConnStatus::Connecting => "CONNECTING",
            ConnStatus::Open => "STREAMING",
            ConnStatus::Reconnecting => "RECONNECTING",
            ConnStatus::Closed => "OFFLINE",
        }
    }

    pub fn dot(&self) -> &'static str {
        match self {
            ConnStatus::Connecting => "bg-amber-400 dot-busy",
            ConnStatus::Open => "bg-emerald-400",
            ConnStatus::Reconnecting => "bg-amber-400 dot-busy",
            ConnStatus::Closed => "bg-rose-500",
        }
    }
}

// ---------------------------------------------------------------------------
// Wire types (see docs/api.md)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Incident {
    pub id: i64,
    pub correlation_key: String,
    pub status: String,
    pub severity: String,
    pub title: String,
    pub summary: String,
    pub source: String,
    pub entity: String,
    pub alert_count: u32,
    pub first_seen_ms: f64,
    pub last_seen_ms: f64,
    pub assignee: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Incident {
    pub fn sev(&self) -> Severity {
        Severity::parse(&self.severity)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Alert {
    pub id: i64,
    pub incident_id: i64,
    pub source: String,
    pub kind: String,
    pub severity: String,
    pub entity: String,
    pub detail: serde_json::Value,
    pub correlation_key: String,
    pub received_at: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TimelineEvent {
    pub id: i64,
    pub incident_id: i64,
    pub run_id: Option<i64>,
    pub actor: String,
    pub kind: String,
    pub payload: serde_json::Value,
    pub ts_ms: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Run {
    pub id: i64,
    pub playbook_id: i64,
    pub incident_id: i64,
    pub status: String,
    pub initiator: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RunStep {
    pub id: i64,
    pub run_id: i64,
    pub seq: i64,
    pub name: String,
    pub status: String,
    pub gate: bool,
    pub gate_status: String,
    pub output: serde_json::Value,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Playbook {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub auto_severity: String,
    pub steps: serde_json::Value,
    pub enabled: bool,
    pub created_by: String,
}

#[derive(Clone, Debug)]
pub struct Toast {
    pub id: u64,
    pub title: String,
    pub detail: String,
    pub critical: bool,
}

// ---------------------------------------------------------------------------
// Signals
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct Metrics {
    /// Session totals mirrored from list responses.
    pub incident_total: RwSignal<u32>,
    /// Monotonic id source for toasts.
    pub next_id: Rc<Cell<u64>>,
}

#[derive(Clone)]
pub struct AppState {
    pub conn: RwSignal<ConnStatus>,

    /// Incident queue (severity-sorted on write).
    pub incidents: SignalVec<Incident>,
    /// Filtered view of the queue.
    pub visible: SignalVec<Incident>,
    pub filter_status: RwSignal<String>,
    pub filter_severity: RwSignal<String>,
    pub filter_query: RwSignal<String>,
    pub selected_id: RwSignal<Option<i64>>,

    /// Selected incident's dossier.
    pub dossier_alerts: SignalVec<Alert>,
    pub dossier_timeline: SignalVec<TimelineEvent>,
    pub dossier_runs: SignalVec<Run>,

    pub playbooks: SignalVec<Playbook>,
    pub toasts: SignalVec<Toast>,

    /// 401/403 from any endpoint (bad key or banned viewer).
    pub banned: RwSignal<bool>,
    pub api_error: RwSignal<String>,
    /// Connector names from `/health`.
    pub connectors: RwSignal<String>,

    pub metrics: Metrics,
}

impl AppState {
    pub fn new() -> Self {
        let app = Self {
            conn: signal!(ConnStatus::Connecting),
            incidents: signal_vec(Vec::new()),
            visible: signal_vec(Vec::new()),
            filter_status: signal!(String::new()),
            filter_severity: signal!(String::new()),
            filter_query: signal!(String::new()),
            selected_id: signal!(None),
            dossier_alerts: signal_vec(Vec::new()),
            dossier_timeline: signal_vec(Vec::new()),
            dossier_runs: signal_vec(Vec::new()),
            playbooks: signal_vec(Vec::new()),
            toasts: signal_vec(Vec::new()),
            banned: signal!(false),
            api_error: signal!(String::new()),
            connectors: signal!(String::new()),
            metrics: Metrics {
                incident_total: signal!(0u32),
                next_id: Rc::new(Cell::new(1)),
            },
        };

        app.install_filter();
        app
    }

    /// Derive `visible` from `incidents` + filters. Severity-sorted already
    /// on write; filters only narrow.
    fn install_filter(&self) {
        let incidents = self.incidents.clone();
        let visible = self.visible.clone();
        let status = self.filter_status;
        let severity = self.filter_severity;
        let query = self.filter_query;

        effect!(move || {
            let want_status = status.get();
            let want_sev = severity.get();
            let q = query.get().to_ascii_lowercase();
            let source = incidents.get();

            let out: Vec<Incident> = source
                .into_iter()
                .filter(|e| want_status.is_empty() || e.status == want_status)
                .filter(|e| want_sev.is_empty() || e.severity == want_sev)
                .filter(|e| {
                    q.is_empty()
                        || e.title.to_ascii_lowercase().contains(&q)
                        || e.entity.to_ascii_lowercase().contains(&q)
                        || e.correlation_key.to_ascii_lowercase().contains(&q)
                })
                .collect();

            visible.with_mut(|v| *v = out);
        });
    }

    /// Insert or replace an incident, keeping severity-then-recency order.
    pub fn upsert_incident(&self, inc: Incident) {
        self.incidents.with_mut(|v| {
            if let Some(pos) = v.iter().position(|e| e.id == inc.id) {
                v[pos] = inc;
            } else {
                v.push(inc);
            }
            v.sort_by(|a, b| {
                Severity::parse(&b.severity)
                    .rank()
                    .cmp(&Severity::parse(&a.severity).rank())
                    .then_with(|| {
                        b.last_seen_ms
                            .partial_cmp(&a.last_seen_ms)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
            });
            if v.len() > INCIDENT_KEEP {
                v.truncate(INCIDENT_KEEP);
            }
        });
    }

    pub fn next_toast_id(&self) -> u64 {
        let id = self.metrics.next_id.get();
        self.metrics.next_id.set(id + 1);
        id
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// `HH:MM:SS` in the browser's local timezone (epoch millis float).
pub fn format_time(ts_ms: f64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ts_ms));
    format!(
        "{:02}:{:02}:{:02}",
        d.get_hours(),
        d.get_minutes(),
        d.get_seconds()
    )
}

/// `YYYY-MM-DD HH:MM` in the browser's local timezone.
pub fn format_date_time(ts_ms: f64) -> String {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ts_ms));
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        d.get_full_year(),
        d.get_month() + 1,
        d.get_date(),
        d.get_hours(),
        d.get_minutes()
    )
}

/// Compact JSON for payload previews.
pub fn short_json(v: &serde_json::Value, max: usize) -> String {
    let s = serde_json::to_string(v).unwrap_or_default();
    if s.len() <= max {
        s
    } else {
        format!("{}…", &s[..max])
    }
}
