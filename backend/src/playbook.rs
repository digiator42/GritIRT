use std::sync::Arc;

use serde_json::{json, Value};

use crate::db::{self, StepGateStatus};
use crate::state::AppState;
use crate::types::{now_ms_f64, StepAction};

/// Start a playbook run for an incident. Materialises the run + steps, then
/// drives execution in a background task. Returns the new run id.
pub async fn start_run(
    st: &Arc<AppState>,
    playbook_id: i64,
    incident_id: i64,
    initiator: &str,
) -> Result<i64, String> {
    let def = db::find_playbook(st.db.as_ref(), playbook_id)
        .await?
        .ok_or_else(|| "playbook_not_found".to_string())?;

    let run_id = db::insert_run(st.db.as_ref(), playbook_id, incident_id, initiator).await?;
    db::create_run_steps(st.db.as_ref(), run_id, &def.steps).await?;

    db::push_timeline(
        st.db.as_ref(),
        incident_id,
        run_id,
        initiator,
        "run_started",
        json!({ "playbook": def.name, "playbook_id": playbook_id, "steps": def.steps.len() }),
    )
    .await?;
    st.feed
        .publish("run", json!({ "id": run_id, "incident_id": incident_id, "status": "running", "playbook": def.name }))
        .await;

    drive(st.clone(), run_id, incident_id).await;
    Ok(run_id)
}

/// Resume a run after an approval decision.
pub async fn resume_run(st: &Arc<AppState>, run_id: i64, incident_id: i64) {
    drive(st.clone(), run_id, incident_id).await;
}

/// Drive a run from the first non-terminal step. Safe to call only when the
/// run is not already active (guarded by `active_runs`).
pub async fn drive(st: Arc<AppState>, run_id: i64, incident_id: i64) {
    {
        let mut active = st.active_runs.lock().await;
        if !active.insert(run_id) {
            // Already being driven by another task; the driver will pick up
            // newly-approved state on its next iteration.
            return;
        }
    }

    let result = drive_inner(&st, run_id, incident_id).await;

    let mut active = st.active_runs.lock().await;
    active.remove(&run_id);

    if let Err(e) = result {
        let _ = db::set_run_status(st.db.as_ref(), run_id, "failed").await;
        let _ = db::push_timeline(
            st.db.as_ref(),
            incident_id,
            run_id,
            "system",
            "run_failed",
            json!({ "error": e }),
        )
        .await;
        let _ = db::find_run(st.db.as_ref(), run_id).await.map(|r| {
            if let Some(run) = r {
                let st2 = st.clone();
                tokio::spawn(async move {
                    st2.feed.publish("run", json!(run)).await;
                });
            }
        });
    }
}

async fn drive_inner(st: &Arc<AppState>, run_id: i64, incident_id: i64) -> Result<(), String> {
    db::set_run_status(st.db.as_ref(), run_id, "running").await?;

    let run = db::find_run(st.db.as_ref(), run_id)
        .await?
        .ok_or_else(|| "run_not_found".to_string())?;
    let def = db::find_playbook(st.db.as_ref(), run.playbook_id).await?;

    let steps = db::list_run_steps(st.db.as_ref(), run_id).await?;

    for step_row in steps {
        if step_row.status != "pending" {
            continue;
        }

        let def_step = def
            .as_ref()
            .and_then(|d| d.steps.get(step_row.seq as usize))
            .cloned();

        // Gated step: pause the run until an analyst decides.
        if step_row.gate && step_row.gate_status == "open" {
            db::set_run_status(st.db.as_ref(), run_id, "waiting_approval").await?;
            db::push_timeline(
                st.db.as_ref(),
                incident_id,
                run_id,
                "system",
                "awaiting_approval",
                json!({ "step": step_row.name, "seq": step_row.seq }),
            )
            .await?;
            st.feed
                .publish(
                    "run",
                    json!({ "id": run_id, "incident_id": incident_id, "status": "waiting_approval", "step": step_row.name, "seq": step_row.seq }),
                )
                .await;
            return Ok(());
        }

        // Gated step that an analyst rejected: stop the whole run.
        if step_row.gate && step_row.gate_status == "rejected" {
            db::finish_step(st.db.as_ref(), step_row.id, false, json!({ "rejected": true })).await?;
            db::cancel_pending_steps(st.db.as_ref(), run_id, step_row.seq).await?;
            db::push_timeline(
                st.db.as_ref(),
                incident_id,
                run_id,
                "analyst",
                "step_rejected",
                json!({ "step": step_row.name, "seq": step_row.seq }),
            )
            .await?;
            finish_run(st, run_id, incident_id, "cancelled").await?;
            return Ok(());
        }

        db::start_step(st.db.as_ref(), step_row.id).await?;
        db::push_timeline(
            st.db.as_ref(),
            incident_id,
            run_id,
            "system",
            "step_started",
            json!({ "step": step_row.name, "seq": step_row.seq }),
        )
        .await?;

        let action = def_step.map(|s| s.action).unwrap_or(StepAction::Sim {
            text: "no-op: step definition missing".to_string(),
        });

        match execute_action(st, incident_id, &action).await {
            Ok(output) => {
                db::finish_step(st.db.as_ref(), step_row.id, true, output.clone()).await?;
                db::push_timeline(
                    st.db.as_ref(),
                    incident_id,
                    run_id,
                    "system",
                    "step_succeeded",
                    json!({ "step": step_row.name, "seq": step_row.seq, "output": output }),
                )
                .await?;
                st.feed
                    .publish(
                        "step",
                        json!({ "run_id": run_id, "incident_id": incident_id, "seq": step_row.seq, "name": step_row.name, "status": "succeeded", "output": output }),
                    )
                    .await;
            }
            Err(e) => {
                db::finish_step(st.db.as_ref(), step_row.id, false, json!({ "error": e })).await?;
                db::push_timeline(
                    st.db.as_ref(),
                    incident_id,
                    run_id,
                    "system",
                    "step_failed",
                    json!({ "step": step_row.name, "seq": step_row.seq, "error": e }),
                )
                .await?;
                st.feed
                    .publish(
                        "step",
                        json!({ "run_id": run_id, "incident_id": incident_id, "seq": step_row.seq, "name": step_row.name, "status": "failed", "error": e }),
                    )
                    .await;
                finish_run(st, run_id, incident_id, "failed").await?;
                return Ok(());
            }
        }
    }

    finish_run(st, run_id, incident_id, "succeeded").await?;
    Ok(())
}

async fn finish_run(
    st: &Arc<AppState>,
    run_id: i64,
    incident_id: i64,
    status: &str,
) -> Result<(), String> {
    db::set_run_status(st.db.as_ref(), run_id, status).await?;
    db::push_timeline(
        st.db.as_ref(),
        incident_id,
        run_id,
        "system",
        "run_finished",
        json!({ "status": status }),
    )
    .await?;
    if let Some(run) = db::find_run(st.db.as_ref(), run_id).await? {
        st.feed.publish("run", json!(run)).await;
    }
    Ok(())
}

/// Record an analyst's decision on a gated step and, when approved, resume the
/// run.
pub async fn decide_gate(
    st: &Arc<AppState>,
    run_id: i64,
    seq: i64,
    approve: bool,
) -> Result<(), String> {
    let run = db::find_run(st.db.as_ref(), run_id)
        .await?
        .ok_or_else(|| "run_not_found".to_string())?;

    let decision = if approve {
        StepGateStatus::Approve
    } else {
        StepGateStatus::Reject
    };
    db::set_step_gate(st.db.as_ref(), run_id, seq, decision).await?;

    db::push_timeline(
        st.db.as_ref(),
        run.incident_id,
        run_id,
        "analyst",
        if approve { "step_approved" } else { "step_rejected" },
        json!({ "seq": seq }),
    )
    .await?;

    // Whether approved or rejected, let the driver make the next move: on
    // approval it runs the gated step; on rejection it stops the run.
    resume_run(st, run_id, run.incident_id).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Action execution
// ---------------------------------------------------------------------------

/// Execute a single step action, returning its JSON output.
async fn execute_action(
    st: &Arc<AppState>,
    incident_id: i64,
    action: &StepAction,
) -> Result<Value, String> {
    match action {
        StepAction::Sim { text } => Ok(json!({ "simulated": true, "text": text })),

        StepAction::Mcp {
            connector,
            tool,
            arguments,
        } => {
            let args = template_value(arguments, st, incident_id).await?;
            call_mcp_tool(st, connector, tool, args).await
        }

        StepAction::Http {
            url,
            method,
            headers,
            body,
        } => {
            let rendered = render_template(url, st, incident_id).await?;
            let body = template_value(body, st, incident_id).await?;
            call_http(st, method_or_default(method), &rendered, headers, body).await
        }
    }
}

fn method_or_default(method: &str) -> &str {
    let m = method.trim();
    if m.is_empty() {
        "POST"
    } else {
        m
    }
}

/// Send a JSON-RPC `tools/call` to a named connector's MCP endpoint.
async fn call_mcp_tool(
    st: &Arc<AppState>,
    connector: &str,
    tool: &str,
    arguments: Value,
) -> Result<Value, String> {
    let conn = st
        .connectors
        .get(connector)
        .ok_or_else(|| format!("unknown_connector:{connector}"))?;

    let url = format!("{}/mcp", conn.base_url.trim_end_matches('/'));
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": tool, "arguments": arguments }
    });

    let mut req = st
        .client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&payload);
    if let Some(key) = &conn.api_key {
        req = req.header("X-API-Key", key);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| format!("connector_request_failed:{e}"))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("connector_read_failed:{e}"))?;

    // The MCP server may answer with plain JSON or an SSE frame; accept both.
    let body = parse_mcp_body(&text);
    if !status.is_success() {
        return Err(format!("connector_http_{}:{}", status.as_u16(), body));
    }

    Ok(json!({ "connector": connector, "tool": tool, "result": body }))
}

/// Generic HTTP JSON action.
async fn call_http(
    st: &Arc<AppState>,
    method: &str,
    url: &str,
    headers: &std::collections::BTreeMap<String, String>,
    body: Value,
) -> Result<Value, String> {
    let m = reqwest::Method::from_bytes(method.to_ascii_uppercase().as_bytes())
        .map_err(|e| format!("bad_method:{e}"))?;
    let mut req = st.client.request(m, url).json(&body);
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let resp = req.send().await.map_err(|e| format!("http_request_failed:{e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("http_read_failed:{e}"))?;
    let parsed = serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!(text));
    if !status.is_success() {
        return Err(format!("http_{}:{}", status.as_u16(), parsed));
    }
    Ok(json!({ "status": status.as_u16(), "body": parsed }))
}

/// Extract a JSON value from an MCP response that may be plain JSON or an SSE
/// `data:` frame.
fn parse_mcp_body(text: &str) -> Value {
    let trimmed = text.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return serde_json::from_str(trimmed).unwrap_or_else(|_| json!(trimmed));
    }
    for line in trimmed.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            let candidate = rest.trim();
            if candidate.starts_with('{') || candidate.starts_with('[') {
                if let Ok(v) = serde_json::from_str::<Value>(candidate) {
                    return v;
                }
            }
        }
    }
    json!(trimmed)
}

// ---------------------------------------------------------------------------
// Templating: replace {{incident.entity}}, {{incident.id}}, {{now}}
// ---------------------------------------------------------------------------

async fn template_value(
    v: &Value,
    st: &Arc<AppState>,
    incident_id: i64,
) -> Result<Value, String> {
    match v {
        Value::String(s) => Ok(Value::String(render_template(s, st, incident_id).await?)),
        Value::Array(arr) => {
            let mut out = Vec::with_capacity(arr.len());
            for item in arr {
                out.push(Box::pin(template_value(item, st, incident_id)).await?);
            }
            Ok(Value::Array(out))
        }
        Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, val) in map {
                out.insert(k.clone(), Box::pin(template_value(val, st, incident_id)).await?);
            }
            Ok(Value::Object(out))
        }
        other => Ok(other.clone()),
    }
}

async fn render_template(s: &str, st: &Arc<AppState>, incident_id: i64) -> Result<String, String> {
    let mut out = s.to_string();

    if out.contains("{{incident.") {
        let inc = db::find_incident(st.db.as_ref(), incident_id)
            .await?
            .ok_or_else(|| "incident_not_found".to_string())?;
        out = out
            .replace("{{incident.id}}", &inc.id.to_string())
            .replace("{{incident.entity}}", &inc.entity)
            .replace("{{incident.title}}", &inc.title)
            .replace("{{incident.severity}}", &inc.severity)
            .replace("{{incident.status}}", &inc.status)
            .replace("{{incident.source}}", &inc.source);
    }

    out = out.replace("{{now_ms}}", &format!("{:.0}", now_ms_f64()));
    Ok(out)
}