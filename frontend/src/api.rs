//! GritIRT backend API contract (see `docs/api.md`; port 8030 via `IRT_PORT`).
//!
//! Single place for every backend assumption: routes, shapes, auth, and the
//! SSE stream URL. Transport is Veloasm's `request()` builder throughout.

use serde::Deserialize;

use crate::state::{Alert, Incident, Playbook, Run, RunStep, TimelineEvent};

/// Backend base URL (dev). Empty means same-origin.
pub const BASE_URL: &str = "http://127.0.0.1:8030";

pub const STREAM_PATH: &str = "/api/v1/stream";
pub const HEALTH_PATH: &str = "/health";
pub const INCIDENTS_PATH: &str = "/api/v1/incidents";
pub const PLAYBOOKS_PATH: &str = "/api/v1/playbooks";
pub const RUNS_PATH: &str = "/api/v1/runs";

/// API key (set when `IRT_API_KEY` is set). `EventSource` cannot send
/// headers, so the key travels as `?api_key=`.
pub const API_KEY: &str = "";

fn with_base(path: &str) -> String {
    if BASE_URL.is_empty() {
        path.to_string()
    } else {
        format!("{}{}", BASE_URL.trim_end_matches('/'), path)
    }
}

fn authenticated(url: String) -> veloasm::RequestBuilder {
    let req = veloasm::request(&url);
    if API_KEY.is_empty() {
        req
    } else {
        req.query("api_key", API_KEY)
    }
}

/// Fully-qualified SSE URL including history replay + auth.
pub fn stream_url() -> String {
    authenticated(with_base(STREAM_PATH))
        .query("history", "100")
        .url()
}

/// Fetch failure with the HTTP status preserved (401/403 = bad key or banned).
#[derive(Debug)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
}

impl ApiError {
    pub fn is_forbidden(&self) -> bool {
        self.status == 401 || self.status == 403
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl From<veloasm::FetchError> for ApiError {
    fn from(e: veloasm::FetchError) -> Self {
        match e {
            veloasm::FetchError::Status { code, .. } if code == 401 || code == 403 => {
                ApiError {
                    status: code,
                    message: format!("forbidden ({code}): bad key or this viewer is banned"),
                }
            }
            veloasm::FetchError::Status { code, reason } => ApiError {
                status: code,
                message: format!("request failed ({code} {reason})"),
            },
            veloasm::FetchError::Network(msg) => ApiError { status: 0, message: msg },
            veloasm::FetchError::Decode(msg) => ApiError { status: 0, message: msg },
        }
    }
}

// ---------------------------------------------------------------------------
// Response shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct HealthReply {
    /// Service status string (kept for contract fidelity; UI keys off HTTP).
    #[allow(dead_code)]
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub connectors: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct IncidentListReply {
    #[serde(default)]
    pub count: u32,
    #[serde(default)]
    pub incidents: Vec<Incident>,
}

#[derive(Debug, Deserialize)]
pub struct DossierReply {
    #[serde(default)]
    pub incident: Incident,
    #[serde(default)]
    pub alerts: Vec<Alert>,
    #[serde(default)]
    pub timeline: Vec<TimelineEvent>,
    #[serde(default)]
    pub runs: Vec<Run>,
}

#[derive(Debug, Deserialize)]
pub struct RunDetailReply {
    /// Run header (kept for contract fidelity; UI renders `steps`).
    #[allow(dead_code)]
    #[serde(default)]
    pub run: Run,
    #[serde(default)]
    pub steps: Vec<RunStep>,
}

#[derive(Debug, Deserialize)]
struct PlaybooksReply {
    #[serde(default)]
    pub playbooks: Vec<Playbook>,
}

#[derive(Debug, Deserialize)]
pub struct PostmortemReply {
    /// Row id (kept for contract fidelity; UI renders `content`).
    #[allow(dead_code)]
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub content: String,
}

#[derive(Debug, Deserialize)]
struct IdReply {
    #[serde(default)]
    pub event_id: i64,
}

#[derive(Debug, Deserialize)]
pub struct RunStartReply {
    #[serde(default)]
    pub run_id: i64,
}

#[derive(Debug, Deserialize)]
pub struct PlaybookCreateReply {
    #[serde(default)]
    pub playbook_id: i64,
}

// ---------------------------------------------------------------------------
// Calls
// ---------------------------------------------------------------------------

pub async fn get_health() -> Result<HealthReply, ApiError> {
    Ok(veloasm::fetch_json(&authenticated(with_base(HEALTH_PATH)).url()).await?)
}

pub async fn list_incidents(
    limit: u32,
    status: &str,
    severity: &str,
) -> Result<(u32, Vec<Incident>), ApiError> {
    let mut req = authenticated(with_base(INCIDENTS_PATH)).query("limit", &limit.to_string());
    if !status.is_empty() {
        req = req.query("status", status);
    }
    if !severity.is_empty() {
        req = req.query("severity", severity);
    }
    let reply: IncidentListReply = veloasm::fetch_json(&req.url()).await?;
    Ok((reply.count, reply.incidents))
}

pub async fn get_dossier(id: i64) -> Result<DossierReply, ApiError> {
    Ok(veloasm::fetch_json(&authenticated(format!("{}/{id}", with_base(INCIDENTS_PATH))).url())
        .await?)
}

pub async fn set_status(id: i64, status: &str) -> Result<Incident, ApiError> {
    #[derive(Deserialize)]
    struct StatusReply {
        incident: Incident,
    }
    let body = serde_json::json!({ "status": status });
    let reply: StatusReply = veloasm::post_json(
        &authenticated(format!("{}/{id}/status", with_base(INCIDENTS_PATH))).url(),
        &body,
    )
    .await?;
    Ok(reply.incident)
}

pub async fn assign(id: i64, assignee: &str) -> Result<Incident, ApiError> {
    #[derive(Deserialize)]
    struct AssignReply {
        incident: Incident,
    }
    let body = serde_json::json!({ "assignee": assignee });
    let reply: AssignReply = veloasm::post_json(
        &authenticated(format!("{}/{id}/assign", with_base(INCIDENTS_PATH))).url(),
        &body,
    )
    .await?;
    Ok(reply.incident)
}

pub async fn add_note(id: i64, text: &str) -> Result<i64, ApiError> {
    let body = serde_json::json!({ "text": text });
    let reply: IdReply = veloasm::post_json(
        &authenticated(format!("{}/{id}/notes", with_base(INCIDENTS_PATH))).url(),
        &body,
    )
    .await?;
    Ok(reply.event_id)
}

pub async fn start_run(id: i64, playbook_id: Option<i64>) -> Result<i64, ApiError> {
    let body = match playbook_id {
        Some(pid) => serde_json::json!({ "playbook_id": pid }),
        None => serde_json::json!({}),
    };
    let reply: RunStartReply = veloasm::post_json(
        &authenticated(format!("{}/{id}/playbook", with_base(INCIDENTS_PATH))).url(),
        &body,
    )
    .await?;
    Ok(reply.run_id)
}

pub async fn get_run_detail(run_id: i64) -> Result<RunDetailReply, ApiError> {
    Ok(veloasm::fetch_json(
        &authenticated(format!("{}/{run_id}", with_base(RUNS_PATH))).url(),
    )
    .await?)
}

pub async fn approve_step(run_id: i64, seq: i64, approve: bool) -> Result<(), ApiError> {
    let body = serde_json::json!({ "seq": seq, "approve": approve });
    let _: serde_json::Value = veloasm::post_json(
        &authenticated(format!("{}/{run_id}/approve", with_base(RUNS_PATH))).url(),
        &body,
    )
    .await?;
    Ok(())
}

pub async fn list_playbooks() -> Result<Vec<Playbook>, ApiError> {
    let reply: PlaybooksReply =
        veloasm::fetch_json(&authenticated(with_base(PLAYBOOKS_PATH)).url()).await?;
    Ok(reply.playbooks)
}

pub async fn create_playbook(
    name: &str,
    description: &str,
    auto_severity: &str,
    steps: &serde_json::Value,
) -> Result<i64, ApiError> {
    let body = serde_json::json!({
        "name": name,
        "description": description,
        "auto_severity": auto_severity,
        "steps": steps,
    });
    let reply: PlaybookCreateReply =
        veloasm::post_json(&authenticated(with_base(PLAYBOOKS_PATH)).url(), &body).await?;
    Ok(reply.playbook_id)
}

pub async fn get_postmortem(id: i64) -> Result<PostmortemReply, ApiError> {
    Ok(veloasm::fetch_json(
        &authenticated(format!("{}/{id}/postmortem", with_base(INCIDENTS_PATH))).url(),
    )
    .await?)
}

pub async fn create_postmortem(id: i64) -> Result<String, ApiError> {
    #[derive(Deserialize)]
    struct CreateReply {
        #[serde(default)]
        content: String,
    }
    let body = serde_json::json!({});
    let reply: CreateReply = veloasm::post_json(
        &authenticated(format!("{}/{id}/postmortem", with_base(INCIDENTS_PATH))).url(),
        &body,
    )
    .await?;
    Ok(reply.content)
}
