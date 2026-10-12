use gritshield::mcp::prompt::McpPromptMessage;
use gritshield::mcp_prompt;
use gritshield::mcp_tool;
use gritshield::routing::engine::RequestContext;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::db;
use crate::ingest;
use crate::playbook;
use crate::state;
use crate::types::{normalize_severity, AlertInput};

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct IngestAlertArgs {
    pub source: String,
    pub severity: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub entity: String,
    #[serde(default)]
    pub detail: Value,
    #[serde(default)]
    pub correlation_key: Option<String>,
}

/// `tools/call ingest_alert` — correlate a new alert into an incident and
/// auto-start a playbook when the severity threshold is met.
#[mcp_tool(
    name = "ingest_alert",
    service = "GritIRT",
    description = "Ingest an alert from any probe (CyberFlow, LuxShield, ACC), correlate it into an incident, and auto-trigger a matching playbook",
    schema = r#"{
        "type": "object",
        "properties": {
            "source":          { "type": "string", "description": "Originating system, e.g. luxshield, cyberflow, acc" },
            "severity":        { "type": "string", "enum": ["low", "medium", "high", "critical"] },
            "kind":            { "type": "string", "description": "Alert type, e.g. port_scan, brute_force" },
            "entity":          { "type": "string", "description": "Primary affected entity: IP, host, or service" },
            "detail":          { "type": "object", "description": "Free-form alert payload" },
            "correlation_key": { "type": "string", "description": "Explicit correlation key; defaults to source:kind:entity" }
        },
        "required": ["source", "severity"]
    }"#,
)]
pub async fn ingest_alert_tool(_ctx: &RequestContext, args: IngestAlertArgs) -> Result<Value, String> {
    let st = state::resolve();
    let input = AlertInput {
        source: args.source,
        kind: args.kind,
        severity: normalize_severity(&args.severity),
        entity: args.entity,
        detail: args.detail,
        ts_ms: 0.0,
        correlation_key: args.correlation_key,
    };

    let outcome = ingest::ingest_alert(&st, input).await?;

    let mut auto_run: Option<i64> = None;
    if outcome.created_incident {
        if let Ok(Some(pb)) = db::pick_auto_playbook(st.db.as_ref(), &outcome.incident.severity).await {
            if let Ok(run_id) = playbook::start_run(&st, pb.id, outcome.incident.id, "auto").await {
                auto_run = Some(run_id);
            }
        }
    }

    Ok(json!({
        "incident": outcome.incident,
        "alert": outcome.alert,
        "created_incident": outcome.created_incident,
        "auto_run_id": auto_run,
    }))
}

#[derive(Debug, Deserialize)]
pub struct ListIncidentsArgs {
    #[serde(default)]
    pub limit: u32,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub severity: Option<String>,
}

/// `tools/call list_incidents` — query the incident queue.
#[mcp_tool(
    name = "list_incidents",
    service = "GritIRT",
    description = "List incidents newest-first, optionally filtered by status and severity",
    schema = r#"{
        "type": "object",
        "properties": {
            "limit":    { "type": "integer", "description": "Max incidents to return", "minimum": 1, "maximum": 500 },
            "status":   { "type": "string", "enum": ["open", "triaging", "investigating", "contained", "eradicating", "recovering", "resolved", "closed"] },
            "severity": { "type": "string", "enum": ["low", "medium", "high", "critical"] }
        },
        "required": []
    }"#,
)]
pub async fn list_incidents_tool(_ctx: &RequestContext, args: ListIncidentsArgs) -> Result<Value, String> {
    let st = state::resolve();
    let limit = if args.limit == 0 { 50 } else { args.limit.clamp(1, 500) } as u64;
    let rows = db::list_incidents(
        st.db.as_ref(),
        limit,
        args.status.as_deref(),
        args.severity.as_deref(),
    )
    .await?;
    Ok(json!({ "count": rows.len(), "incidents": rows }))
}

#[derive(Debug, Deserialize)]
pub struct IncidentIdArgs {
    pub incident_id: i64,
}

/// `tools/call get_incident` — full incident detail: alerts, timeline, runs.
#[mcp_tool(
    name = "get_incident",
    service = "GritIRT",
    description = "Fetch a full incident dossier: header, correlated alerts, timeline and playbook runs",
    schema = r#"{
        "type": "object",
        "properties": {
            "incident_id": { "type": "integer", "description": "Incident id" }
        },
        "required": ["incident_id"]
    }"#,
)]
pub async fn get_incident_tool(_ctx: &RequestContext, args: IncidentIdArgs) -> Result<Value, String> {
    let st = state::resolve();
    let incident = db::find_incident(st.db.as_ref(), args.incident_id)
        .await?
        .ok_or_else(|| format!("incident {} not found", args.incident_id))?;
    let alerts = db::list_incident_alerts(st.db.as_ref(), args.incident_id).await?;
    let timeline = db::list_timeline(st.db.as_ref(), args.incident_id).await?;
    let runs = db::list_incident_runs(st.db.as_ref(), args.incident_id).await?;
    Ok(json!({ "incident": incident, "alerts": alerts, "timeline": timeline, "runs": runs }))
}

#[derive(Debug, Deserialize)]
pub struct SetStatusArgs {
    pub incident_id: i64,
    pub status: String,
}

/// `tools/call set_incident_status` — drive the incident lifecycle.
#[mcp_tool(
    name = "set_incident_status",
    service = "GritIRT",
    description = "Transition an incident's lifecycle status and record it on the timeline",
    schema = r#"{
        "type": "object",
        "properties": {
            "incident_id": { "type": "integer", "description": "Incident id" },
            "status":      { "type": "string", "enum": ["open", "triaging", "investigating", "contained", "eradicating", "recovering", "resolved", "closed"] }
        },
        "required": ["incident_id", "status"]
    }"#,
)]
pub async fn set_incident_status_tool(_ctx: &RequestContext, args: SetStatusArgs) -> Result<Value, String> {
    let st = state::resolve();
    let inc = db::update_incident_status(st.db.as_ref(), args.incident_id, &args.status).await?;
    db::push_timeline(
        st.db.as_ref(),
        args.incident_id,
        0,
        "mcp",
        "status_changed",
        json!({ "status": inc.status }),
    )
    .await?;
    st.feed.publish("incident", json!(inc)).await;
    Ok(json!({ "incident": inc }))
}

#[derive(Debug, Deserialize)]
pub struct RunPlaybookArgs {
    pub incident_id: i64,
    #[serde(default)]
    pub playbook_id: Option<i64>,
}

/// `tools/call run_playbook` — start a runbook against an incident.
#[mcp_tool(
    name = "run_playbook",
    service = "GritIRT",
    description = "Start a playbook run for an incident (defaults to the first auto-matching playbook for its severity)",
    schema = r#"{
        "type": "object",
        "properties": {
            "incident_id": { "type": "integer", "description": "Incident to run against" },
            "playbook_id": { "type": "integer", "description": "Explicit playbook id; omit to auto-match by severity" }
        },
        "required": ["incident_id"]
    }"#,
)]
pub async fn run_playbook_tool(_ctx: &RequestContext, args: RunPlaybookArgs) -> Result<Value, String> {
    let st = state::resolve();
    let incident = db::find_incident(st.db.as_ref(), args.incident_id)
        .await?
        .ok_or_else(|| format!("incident {} not found", args.incident_id))?;

    let playbook_id = match args.playbook_id {
        Some(pb) => pb,
        None => db::pick_auto_playbook(st.db.as_ref(), &incident.severity)
            .await?
            .map(|p| p.id)
            .ok_or_else(|| "no playbook matched the incident severity".to_string())?,
    };

    let run_id = playbook::start_run(&st, playbook_id, args.incident_id, "mcp").await?;
    Ok(json!({ "run_id": run_id, "playbook_id": playbook_id }))
}

#[derive(Debug, Deserialize)]
pub struct ApproveArgs {
    pub run_id: i64,
    pub seq: i64,
    #[serde(default = "default_true")]
    pub approve: bool,
}

fn default_true() -> bool {
    true
}

/// `tools/call approve_step` — approve or reject a gated runbook step.
#[mcp_tool(
    name = "approve_step",
    service = "GritIRT",
    description = "Approve or reject a gated playbook step, resuming or stopping the run",
    schema = r#"{
        "type": "object",
        "properties": {
            "run_id":  { "type": "integer", "description": "Playbook run id" },
            "seq":     { "type": "integer", "description": "Zero-based step sequence within the run" },
            "approve": { "type": "boolean", "description": "true to approve (default), false to reject" }
        },
        "required": ["run_id", "seq"]
    }"#,
)]
pub async fn approve_step_tool(_ctx: &RequestContext, args: ApproveArgs) -> Result<Value, String> {
    let st = state::resolve();
    playbook::decide_gate(&st, args.run_id, args.seq, args.approve).await?;
    Ok(json!({ "run_id": args.run_id, "seq": args.seq, "approved": args.approve }))
}

#[derive(Debug, Deserialize)]
pub struct ExportPostmortemArgs {
    pub incident_id: i64,
}

/// `tools/call export_postmortem` — generate a Markdown postmortem document.
#[mcp_tool(
    name = "export_postmortem",
    service = "GritIRT",
    description = "Generate and store a Markdown postmortem for an incident from its alert and response timeline",
    schema = r#"{
        "type": "object",
        "properties": {
            "incident_id": { "type": "integer", "description": "Incident id" }
        },
        "required": ["incident_id"]
    }"#,
)]
pub async fn export_postmortem_tool(
    _ctx: &RequestContext,
    args: ExportPostmortemArgs,
) -> Result<Value, String> {
    let st = state::resolve();
    let incident = db::find_incident(st.db.as_ref(), args.incident_id)
        .await?
        .ok_or_else(|| format!("incident {} not found", args.incident_id))?;
    let alerts = db::list_incident_alerts(st.db.as_ref(), args.incident_id).await?;
    let timeline = db::list_timeline(st.db.as_ref(), args.incident_id).await?;

    let content = crate::api::render_postmortem(&incident, &alerts, &timeline);
    let doc_id = db::insert_postmortem(st.db.as_ref(), args.incident_id, &content).await?;
    Ok(json!({ "postmortem_id": doc_id, "content": content }))
}

// ---------------------------------------------------------------------------
// Prompts
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct TriageArgs {
    pub incident_id: i64,
}

/// `prompts/get triage_incident` — build a triage brief for an incident.
#[mcp_prompt(
    name = "triage_incident",
    description = "Build an analyst-facing triage brief for an incident from its correlated alerts and response timeline",
    arguments = "incident_id",
    descriptions = "incident_id=Incident id to triage"
)]
pub async fn triage_incident(args: TriageArgs) -> Result<Vec<McpPromptMessage>, String> {
    let st = state::resolve();
    let incident = db::find_incident(st.db.as_ref(), args.incident_id)
        .await?
        .ok_or_else(|| format!("incident {} not found", args.incident_id))?;
    let alerts = db::list_incident_alerts(st.db.as_ref(), args.incident_id).await?;
    let timeline = db::list_timeline(st.db.as_ref(), args.incident_id).await?;

    let alert_lines = alerts
        .iter()
        .rev()
        .take(20)
        .map(|a| format!("- {} {} [{}] {}: {}", a.received_at, a.severity, a.source, a.kind, a.entity))
        .collect::<Vec<_>>()
        .join("\n");

    let timeline_lines = timeline
        .iter()
        .rev()
        .take(20)
        .map(|e| format!("- [{}] {} ({}) {}", e.ts_ms, e.kind, e.actor, e.payload))
        .collect::<Vec<_>>()
        .join("\n");

    let message = format!(
        "You are an incident-response analyst. Produce a concise triage brief.\n\n\
         Incident #{} \"{}\" — severity {}, status {}, {} correlated alerts against {}.\n\n\
         Recent alerts:\n{}\n\nRecent response timeline:\n{}\n\n\
         Include: what likely happened, blast radius, confidence, recommended containment actions, \
         and whether to escalate.",
        incident.id, incident.title, incident.severity, incident.status, incident.alert_count,
        incident.entity, alert_lines, timeline_lines
    );

    Ok(vec![McpPromptMessage::user(message)])
}