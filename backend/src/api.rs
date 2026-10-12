use gritshield::http::SseStream;
use gritshield::prelude::{RequestContext, Response};
use serde_json::{json, Value};

use crate::db;
use crate::ingest;
use crate::playbook;
use crate::state;
use crate::types::{AlertInput, PlaybookDef};

/// `GET /health` — liveness plus a peek at stored counts.
pub async fn health(_ctx: RequestContext) -> Response {
    let st = state::resolve();
    let db_ok = db::ping(st.db.as_ref()).await.is_ok();
    let (history_len, stream_count, received_total, attached_total) = st.feed.stats().await;

    Response::json_ok(&json!({
        "service": "gritirt-backend",
        "status": "ok",
        "db": db_ok,
        "feed": {
            "history_count": history_len,
            "history_capacity": crate::feed::HISTORY_CAPACITY,
            "live_streams": stream_count,
            "published_total": received_total,
            "attached_total": attached_total,
        },
        "connectors": st.connectors.keys().cloned().collect::<Vec<_>>(),
    }))
}

/// `POST /api/v1/alerts` — ingest an alert, correlate it into an incident, and
/// auto-start a playbook when the incident severity meets a playbook threshold.
///
/// Body:
/// ```json
/// {
///   "source": "luxshield",
///   "kind": "port_scan",
///   "severity": "high",
///   "entity": "203.0.113.9",
///   "detail": { "attempts": 512 },
///   "ts_ms": 1720524000000,
///   "correlation_key": null
/// }
/// ```
pub async fn post_alert(ctx: RequestContext) -> Response {
    let st = state::resolve();

    let body: Value = match ctx.json_body().await {
        Some(v) => v,
        None => return Response::json_bad_request(&json!({ "error": "invalid_json" })),
    };

    let input: AlertInput = match serde_json::from_value(body) {
        Ok(input) => input,
        Err(e) => {
            return Response::json_bad_request(&json!({
                "error": "invalid_alert",
                "detail": e.to_string()
            }))
        }
    };

    if input.source.trim().is_empty() {
        return Response::json_bad_request(&json!({ "error": "source_required" }));
    }

    let outcome = match ingest::ingest_alert(&st, input).await {
        Ok(o) => o,
        Err(e) => return Response::json_internal_error_msg(format!("ingest_failed: {e}")),
    };

    // Auto-trigger: only when this alert opened a new incident.
    let mut auto_run: Option<i64> = None;
    if outcome.created_incident {
        if let Ok(Some(pb)) = db::pick_auto_playbook(st.db.as_ref(), &outcome.incident.severity).await {
            if let Ok(run_id) = playbook::start_run(&st, pb.id, outcome.incident.id, "auto").await {
                auto_run = Some(run_id);
            }
        }
    }

    let status = if outcome.created_incident { 201 } else { 200 };
    let mut res = Response::json_ok(&json!({
        "incident": outcome.incident,
        "alert": outcome.alert,
        "created_incident": outcome.created_incident,
        "auto_run_id": auto_run,
    }));
    res.status = status;
    res
}

/// `GET /api/v1/incidents?limit=&status=&severity=`.
pub async fn list_incidents(ctx: RequestContext) -> Response {
    let st = state::resolve();
    let limit = ctx
        .query_param("limit")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(50)
        .clamp(1, 500);
    let status = ctx.query_param("status").map(ToOwned::to_owned);
    let severity = ctx.query_param("severity").map(ToOwned::to_owned);

    match db::list_incidents(st.db.as_ref(), limit, status.as_deref(), severity.as_deref()).await {
        Ok(rows) => Response::json_ok(&json!({ "count": rows.len(), "incidents": rows })),
        Err(e) => Response::json_internal_error_msg(format!("list_failed: {e}")),
    }
}

/// `GET /api/v1/incidents/{id}` — incident + alerts + timeline + runs.
pub async fn get_incident(ctx: RequestContext) -> Response {
    let st = state::resolve();
    let id = match path_id(&ctx) {
        Some(id) => id,
        None => return Response::json_bad_request(&json!({ "error": "invalid_id" })),
    };

    let incident = match db::find_incident(st.db.as_ref(), id).await {
        Ok(Some(inc)) => inc,
        Ok(None) => return Response::json_not_found(&json!({ "error": "incident_not_found", "id": id })),
        Err(e) => return Response::json_internal_error_msg(format!("lookup_failed: {e}")),
    };

    let alerts = db::list_incident_alerts(st.db.as_ref(), id).await.unwrap_or_default();
    let timeline = db::list_timeline(st.db.as_ref(), id).await.unwrap_or_default();
    let runs = db::list_incident_runs(st.db.as_ref(), id).await.unwrap_or_default();

    Response::json_ok(&json!({
        "incident": incident,
        "alerts": alerts,
        "timeline": timeline,
        "runs": runs,
    }))
}

/// `POST /api/v1/incidents/{id}/status` — `{ "status": "investigating" }`.
pub async fn set_status(ctx: RequestContext) -> Response {
    let st = state::resolve();
    let id = match path_id(&ctx) {
        Some(id) => id,
        None => return Response::json_bad_request(&json!({ "error": "invalid_id" })),
    };
    let body: Value = match ctx.json_body().await {
        Some(v) => v,
        None => return Response::json_bad_request(&json!({ "error": "invalid_json" })),
    };
    let status = body.get("status").and_then(|v| v.as_str()).unwrap_or("");
    if status.is_empty() {
        return Response::json_bad_request(&json!({ "error": "status_required" }));
    }

    match db::update_incident_status(st.db.as_ref(), id, status).await {
        Ok(inc) => {
            let _ = db::push_timeline(
                st.db.as_ref(),
                id,
                0,
                "analyst",
                "status_changed",
                json!({ "status": inc.status }),
            )
            .await;
            st.feed.publish("incident", json!(inc)).await;
            Response::json_ok(&json!({ "incident": inc }))
        }
        Err(e) if e == "incident_not_found" => {
            Response::json_not_found(&json!({ "error": "incident_not_found", "id": id }))
        }
        Err(e) => Response::json_internal_error_msg(format!("update_failed: {e}")),
    }
}

/// `POST /api/v1/incidents/{id}/assign` — `{ "assignee": "alice" }`.
pub async fn assign(ctx: RequestContext) -> Response {
    let st = state::resolve();
    let id = match path_id(&ctx) {
        Some(id) => id,
        None => return Response::json_bad_request(&json!({ "error": "invalid_id" })),
    };
    let body: Value = match ctx.json_body().await {
        Some(v) => v,
        None => return Response::json_bad_request(&json!({ "error": "invalid_json" })),
    };
    let assignee = body.get("assignee").and_then(|v| v.as_str()).unwrap_or("");
    if assignee.is_empty() {
        return Response::json_bad_request(&json!({ "error": "assignee_required" }));
    }

    match db::assign_incident(st.db.as_ref(), id, assignee).await {
        Ok(inc) => {
            let _ = db::push_timeline(
                st.db.as_ref(),
                id,
                0,
                "analyst",
                "assigned",
                json!({ "assignee": assignee }),
            )
            .await;
            st.feed.publish("incident", json!(inc)).await;
            Response::json_ok(&json!({ "incident": inc }))
        }
        Err(e) if e == "incident_not_found" => {
            Response::json_not_found(&json!({ "error": "incident_not_found", "id": id }))
        }
        Err(e) => Response::json_internal_error_msg(format!("assign_failed: {e}")),
    }
}

/// `POST /api/v1/incidents/{id}/notes` — `{ "text": "...", "actor": "alice" }`.
pub async fn add_note(ctx: RequestContext) -> Response {
    let st = state::resolve();
    let id = match path_id(&ctx) {
        Some(id) => id,
        None => return Response::json_bad_request(&json!({ "error": "invalid_id" })),
    };
    let body: Value = match ctx.json_body().await {
        Some(v) => v,
        None => return Response::json_bad_request(&json!({ "error": "invalid_json" })),
    };
    let text = body.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let actor = body.get("actor").and_then(|v| v.as_str()).unwrap_or("analyst");
    if text.is_empty() {
        return Response::json_bad_request(&json!({ "error": "text_required" }));
    }

    if db::find_incident(st.db.as_ref(), id).await.ok().flatten().is_none() {
        return Response::json_not_found(&json!({ "error": "incident_not_found", "id": id }));
    }

    match ingest::add_note(&st, id, actor, text).await {
        Ok(event_id) => Response::json_created(&json!({ "event_id": event_id })),
        Err(e) => Response::json_internal_error_msg(format!("note_failed: {e}")),
    }
}

/// `GET /api/v1/incidents/{id}/timeline`.
pub async fn get_timeline(ctx: RequestContext) -> Response {
    let st = state::resolve();
    let id = match path_id(&ctx) {
        Some(id) => id,
        None => return Response::json_bad_request(&json!({ "error": "invalid_id" })),
    };
    match db::list_timeline(st.db.as_ref(), id).await {
        Ok(events) => Response::json_ok(&json!({ "count": events.len(), "timeline": events })),
        Err(e) => Response::json_internal_error_msg(format!("timeline_failed: {e}")),
    }
}

/// `GET /api/v1/incidents/{id}/alerts`.
pub async fn get_alerts(ctx: RequestContext) -> Response {
    let st = state::resolve();
    let id = match path_id(&ctx) {
        Some(id) => id,
        None => return Response::json_bad_request(&json!({ "error": "invalid_id" })),
    };
    match db::list_incident_alerts(st.db.as_ref(), id).await {
        Ok(alerts) => Response::json_ok(&json!({ "count": alerts.len(), "alerts": alerts })),
        Err(e) => Response::json_internal_error_msg(format!("alerts_failed: {e}")),
    }
}

/// `POST /api/v1/incidents/{id}/playbook` — `{ "playbook_id": 3 }` or, when
/// omitted, the first auto-matching playbook for the incident severity.
pub async fn run_playbook(ctx: RequestContext) -> Response {
    let st = state::resolve();
    let id = match path_id(&ctx) {
        Some(id) => id,
        None => return Response::json_bad_request(&json!({ "error": "invalid_id" })),
    };

    let incident = match db::find_incident(st.db.as_ref(), id).await {
        Ok(Some(inc)) => inc,
        Ok(None) => return Response::json_not_found(&json!({ "error": "incident_not_found", "id": id })),
        Err(e) => return Response::json_internal_error_msg(format!("lookup_failed: {e}")),
    };

    let body: Value = ctx.json_body().await.unwrap_or(json!({}));
    let playbook_id = match body.get("playbook_id").and_then(|v| v.as_i64()) {
        Some(pb) => pb,
        None => match db::pick_auto_playbook(st.db.as_ref(), &incident.severity).await {
            Ok(Some(pb)) => pb.id,
            _ => return Response::json_bad_request(&json!({ "error": "no_playbook_matched" })),
        },
    };

    match playbook::start_run(&st, playbook_id, id, "analyst").await {
        Ok(run_id) => Response::json_created(&json!({ "run_id": run_id, "playbook_id": playbook_id })),
        Err(e) if e == "playbook_not_found" => {
            Response::json_not_found(&json!({ "error": "playbook_not_found", "playbook_id": playbook_id }))
        }
        Err(e) => Response::json_internal_error_msg(format!("run_failed: {e}")),
    }
}

/// `GET /api/v1/incidents/{id}/runs`.
pub async fn get_runs(ctx: RequestContext) -> Response {
    let st = state::resolve();
    let id = match path_id(&ctx) {
        Some(id) => id,
        None => return Response::json_bad_request(&json!({ "error": "invalid_id" })),
    };
    match db::list_incident_runs(st.db.as_ref(), id).await {
        Ok(runs) => Response::json_ok(&json!({ "count": runs.len(), "runs": runs })),
        Err(e) => Response::json_internal_error_msg(format!("runs_failed: {e}")),
    }
}

/// `GET /api/v1/runs/{id}` — run header + steps.
pub async fn get_run(ctx: RequestContext) -> Response {
    let st = state::resolve();
    let id = match path_id(&ctx) {
        Some(id) => id,
        None => return Response::json_bad_request(&json!({ "error": "invalid_id" })),
    };
    let run = match db::find_run(st.db.as_ref(), id).await {
        Ok(Some(run)) => run,
        Ok(None) => return Response::json_not_found(&json!({ "error": "run_not_found", "id": id })),
        Err(e) => return Response::json_internal_error_msg(format!("run_lookup_failed: {e}")),
    };
    let steps = db::list_run_steps(st.db.as_ref(), id).await.unwrap_or_default();
    Response::json_ok(&json!({ "run": run, "steps": steps }))
}

/// `POST /api/v1/runs/{id}/approve` — `{ "seq": 2, "approve": true }`.
pub async fn approve_step(ctx: RequestContext) -> Response {
    let st = state::resolve();
    let id = match path_id(&ctx) {
        Some(id) => id,
        None => return Response::json_bad_request(&json!({ "error": "invalid_id" })),
    };
    let body: Value = match ctx.json_body().await {
        Some(v) => v,
        None => return Response::json_bad_request(&json!({ "error": "invalid_json" })),
    };
    let seq = body.get("seq").and_then(|v| v.as_i64());
    let approve = body.get("approve").and_then(|v| v.as_bool()).unwrap_or(true);
    let seq = match seq {
        Some(s) => s,
        None => return Response::json_bad_request(&json!({ "error": "seq_required" })),
    };

    match playbook::decide_gate(&st, id, seq, approve).await {
        Ok(()) => Response::json_ok(&json!({ "run_id": id, "seq": seq, "approved": approve })),
        Err(e) if e == "run_not_found" => {
            Response::json_not_found(&json!({ "error": "run_not_found", "id": id }))
        }
        Err(e) if e == "step_not_found" => {
            Response::json_not_found(&json!({ "error": "step_not_found", "run_id": id, "seq": seq }))
        }
        Err(e) => Response::json_internal_error_msg(format!("approve_failed: {e}")),
    }
}

/// `GET /api/v1/playbooks`.
pub async fn list_playbooks(_ctx: RequestContext) -> Response {
    let st = state::resolve();
    match db::list_playbooks(st.db.as_ref()).await {
        Ok(rows) => Response::json_ok(&json!({ "count": rows.len(), "playbooks": rows })),
        Err(e) => Response::json_internal_error_msg(format!("playbooks_failed: {e}")),
    }
}

/// `POST /api/v1/playbooks` — body is a `PlaybookDef`:
/// ```json
/// {
///   "name": "Block scanner IP",
///   "description": "Blackhole a hostile IP across the fleet",
///   "auto_severity": "high",
///   "steps": [
///     { "name": "note", "action": { "kind": "sim", "text": "triaging" } },
///     { "name": "block", "gate": true,
///       "action": { "kind": "mcp", "connector": "luxshield", "tool": "add_firewall_rule",
///                   "arguments": { "ip": "{{incident.entity}}", "action": "drop" } } }
///   ]
/// }
/// ```
pub async fn create_playbook(ctx: RequestContext) -> Response {
    let st = state::resolve();
    let body: Value = match ctx.json_body().await {
        Some(v) => v,
        None => return Response::json_bad_request(&json!({ "error": "invalid_json" })),
    };

    let def: PlaybookDef = match serde_json::from_value(body) {
        Ok(def) => def,
        Err(e) => {
            return Response::json_bad_request(&json!({
                "error": "invalid_playbook",
                "detail": e.to_string()
            }))
        }
    };

    if def.name.trim().is_empty() {
        return Response::json_bad_request(&json!({ "error": "name_required" }));
    }
    if def.steps.is_empty() {
        return Response::json_bad_request(&json!({ "error": "steps_required" }));
    }

    match db::insert_playbook(st.db.as_ref(), &def).await {
        Ok(id) => Response::json_created(&json!({ "playbook_id": id, "name": def.name })),
        Err(e) => Response::json_internal_error_msg(format!("playbook_create_failed: {e}")),
    }
}

/// `GET /api/v1/playbooks/{id}`.
pub async fn get_playbook(ctx: RequestContext) -> Response {
    let st = state::resolve();
    let id = match path_id(&ctx) {
        Some(id) => id,
        None => return Response::json_bad_request(&json!({ "error": "invalid_id" })),
    };
    match db::find_playbook(st.db.as_ref(), id).await {
        Ok(Some(pb)) => Response::json_ok(&json!({ "playbook": pb })),
        Ok(None) => Response::json_not_found(&json!({ "error": "playbook_not_found", "id": id })),
        Err(e) => Response::json_internal_error_msg(format!("playbook_lookup_failed: {e}")),
    }
}

/// `POST /api/v1/incidents/{id}/postmortem` — generate and store a Markdown
/// postmortem from the incident history.
pub async fn create_postmortem(ctx: RequestContext) -> Response {
    let st = state::resolve();
    let id = match path_id(&ctx) {
        Some(id) => id,
        None => return Response::json_bad_request(&json!({ "error": "invalid_id" })),
    };

    let incident = match db::find_incident(st.db.as_ref(), id).await {
        Ok(Some(inc)) => inc,
        Ok(None) => return Response::json_not_found(&json!({ "error": "incident_not_found", "id": id })),
        Err(e) => return Response::json_internal_error_msg(format!("lookup_failed: {e}")),
    };
    let alerts = db::list_incident_alerts(st.db.as_ref(), id).await.unwrap_or_default();
    let timeline = db::list_timeline(st.db.as_ref(), id).await.unwrap_or_default();

    let content = render_postmortem(&incident, &alerts, &timeline);
    match db::insert_postmortem(st.db.as_ref(), id, &content).await {
        Ok(doc_id) => {
            let _ = db::push_timeline(
                st.db.as_ref(),
                id,
                0,
                "system",
                "postmortem_created",
                json!({ "postmortem_id": doc_id }),
            )
            .await;
            Response::json_created(&json!({ "postmortem_id": doc_id, "content": content }))
        }
        Err(e) => Response::json_internal_error_msg(format!("postmortem_failed: {e}")),
    }
}

/// `GET /api/v1/incidents/{id}/postmortem` — most recently generated document.
pub async fn get_postmortem(ctx: RequestContext) -> Response {
    let st = state::resolve();
    let id = match path_id(&ctx) {
        Some(id) => id,
        None => return Response::json_bad_request(&json!({ "error": "invalid_id" })),
    };
    match db::latest_postmortem(st.db.as_ref(), id).await {
        Ok(Some(pm)) => Response::json_ok(&json!(pm)),
        Ok(None) => Response::json_not_found(&json!({ "error": "postmortem_not_found", "id": id })),
        Err(e) => Response::json_internal_error_msg(format!("postmortem_lookup_failed: {e}")),
    }
}

/// `GET /api/v1/stream` — Server-Sent Events feed of incidents, alerts,
/// timeline entries, runs and steps. Event names: `incident`, `alert`,
/// `timeline`, `run`, `step`.
///
/// Query params: `history` (default 50, max 500), `api_key`.
pub async fn stream(ctx: RequestContext) -> Response {
    let st = state::resolve();

    let limit = ctx
        .query_param("history")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(50)
        .min(500);

    let history = st.feed.history(limit).await;

    let mut sse = SseStream::new();
    for (event, data) in history {
        sse.queue_initial(&event, &data.to_string());
    }

    st.feed.attach(sse.clone()).await;
    Response::sse(sse)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn path_id(ctx: &RequestContext) -> Option<i64> {
    ctx.param("id").and_then(|v| v.parse::<i64>().ok())
}

pub fn render_postmortem(
    incident: &crate::types::IncidentView,
    alerts: &[crate::types::AlertView],
    timeline: &[crate::types::TimelineEventView],
) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Postmortem: {}\n\n", incident.title));
    out.push_str(&format!("- **Incident ID:** {}\n", incident.id));
    out.push_str(&format!("- **Severity:** {}\n", incident.severity));
    out.push_str(&format!("- **Status:** {}\n", incident.status));
    out.push_str(&format!("- **Source:** {}\n", incident.source));
    out.push_str(&format!("- **Entity:** {}\n", incident.entity));
    out.push_str(&format!("- **Correlation key:** `{}`\n", incident.correlation_key));
    out.push_str(&format!("- **Alerts correlated:** {}\n", incident.alert_count));
    out.push_str(&format!("- **First seen:** {:.0}\n", incident.first_seen_ms));
    out.push_str(&format!("- **Last seen:** {:.0}\n\n", incident.last_seen_ms));

    out.push_str("## Alert timeline\n\n");
    for a in alerts.iter().rev() {
        out.push_str(&format!(
            "- `{}` **{}** / {} — {} (severity {})\n",
            a.received_at, a.source, a.kind, a.entity, a.severity
        ));
    }

    out.push_str("\n## Response timeline\n\n");
    for e in timeline {
        out.push_str(&format!("- [{:.0}] **{}** ({}) {}\n", e.ts_ms, e.kind, e.actor, e.payload));
    }

    out.push_str("\n## Follow-ups\n\n- [ ] Root cause identified\n- [ ] Detection gap closed\n- [ ] Runbook updated\n");
    out
}