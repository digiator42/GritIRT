use serde::{Deserialize, Serialize};

/// Current clock as integer epoch milliseconds.
pub fn now_ms_i64() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Current clock as fractional epoch milliseconds (matches SOC timestamps).
pub fn now_ms_f64() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// Canonical alert/incident severities, mapped from any loose input value.
pub fn normalize_severity(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "critical" => "critical".to_string(),
        "high" => "high".to_string(),
        "low" => "low".to_string(),
        _ => "medium".to_string(),
    }
}

/// Rank used for ordering and auto-trigger comparisons.
pub fn severity_rank(sev: &str) -> u8 {
    match normalize_severity(sev).as_str() {
        "critical" => 4,
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 0,
    }
}

/// Incident lifecycle states.
pub fn normalize_status(raw: &str) -> String {
    let s = raw.trim().to_ascii_lowercase();
    match s.as_str() {
        "open" | "triaging" | "investigating" | "contained" | "eradicating" | "recovering"
        | "resolved" | "closed" => s,
        _ => "open".to_string(),
    }
}

/// Runbook/step lifecycle states.
pub fn normalize_run_status(raw: &str) -> String {
    let s = raw.trim().to_ascii_lowercase();
    match s.as_str() {
        "pending" | "running" | "waiting_approval" | "succeeded" | "failed" | "cancelled" => s,
        _ => "pending".to_string(),
    }
}

/// Incoming alert submitted through `POST /api/v1/alerts` by CyberFlow,
/// LuxShield, the ACC or any other probe.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertInput {
    pub source: String,
    #[serde(default)]
    pub kind: String,
    pub severity: String,
    #[serde(default)]
    pub entity: String,
    #[serde(default)]
    pub detail: serde_json::Value,
    #[serde(default)]
    pub ts_ms: f64,
    #[serde(default)]
    pub correlation_key: Option<String>,
}

/// Normalised alert view used across REST/MCP/SSE responses.
#[derive(Debug, Clone, Serialize)]
pub struct AlertView {
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

/// Incident header/row view.
#[derive(Debug, Clone, Serialize)]
pub struct IncidentView {
    pub id: i64,
    pub correlation_key: String,
    pub status: String,
    pub severity: String,
    pub title: String,
    pub summary: String,
    pub source: String,
    pub entity: String,
    pub alert_count: i64,
    pub first_seen_ms: f64,
    pub last_seen_ms: f64,
    pub assignee: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Timeline entry attached to an incident.
#[derive(Debug, Clone, Serialize)]
pub struct TimelineEventView {
    pub id: i64,
    pub incident_id: i64,
    pub run_id: i64,
    pub actor: String,
    pub kind: String,
    pub payload: serde_json::Value,
    pub ts_ms: f64,
}

/// A single step inside a playbook definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybookStep {
    pub name: String,
    #[serde(default)]
    pub action: StepAction,
    /// When `true` the step is gated behind an analyst approval.
    #[serde(default)]
    pub gate: bool,
}

/// What a playbook step does. Connector actions reach out to one of the
/// configured systems through its MCP transport.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StepAction {
    /// Record a note / emit output without calling any external system.
    Sim { text: String },
    /// Call an MCP tool on a named connector (`cyberflow`, `luxshield`, `acc`).
    Mcp {
        connector: String,
        tool: String,
        arguments: serde_json::Value,
    },
    /// Post an HTTP JSON body to a templated URL (e.g. a GritJira REST call).
    Http {
        url: String,
        #[serde(default)]
        method: String,
        #[serde(default)]
        headers: std::collections::BTreeMap<String, String>,
        #[serde(default)]
        body: serde_json::Value,
    },
}

impl Default for StepAction {
    fn default() -> Self {
        StepAction::Sim {
            text: String::new(),
        }
    }
}

/// Playbook definition: when it auto-triggers and the ordered step list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybookDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Minimum severity threshold to auto-trigger ("" = manual only).
    #[serde(default)]
    pub auto_severity: String,
    /// Hard-coded JSON steps (preserved verbatim, engine-internal).
    #[serde(default)]
    pub steps: Vec<PlaybookStep>,
}

impl PlaybookDef {
    pub fn canonical(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "description": self.description,
            "auto_severity": self.auto_severity,
            "steps": self.steps,
        })
    }
}

/// Playbook view returned to clients.
#[derive(Debug, Clone, Serialize)]
pub struct PlaybookView {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub auto_severity: String,
    pub enabled: bool,
    pub steps: Vec<PlaybookStep>,
    pub created_at: i64,
}

/// A playbook execution (run).
#[derive(Debug, Clone, Serialize)]
pub struct RunView {
    pub id: i64,
    pub playbook_id: i64,
    pub incident_id: i64,
    pub status: String,
    pub initiator: String,
    pub started_at: i64,
    pub finished_at: i64,
}

/// One executed step of a run.
#[derive(Debug, Clone, Serialize)]
pub struct RunStepView {
    pub id: i64,
    pub run_id: i64,
    pub seq: i64,
    pub name: String,
    pub status: String,
    pub gate: bool,
    pub gate_status: String,
    pub output: serde_json::Value,
    pub started_at: i64,
    pub finished_at: i64,
}

/// Postmortem document rendered for an incident.
#[derive(Debug, Clone, Serialize)]
pub struct PostmortemView {
    pub id: i64,
    pub incident_id: i64,
    pub content: String,
    pub created_at: i64,
}