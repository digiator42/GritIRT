use sea_orm::ActiveValue::{NotSet, Set};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QueryOrder, QuerySelect,
};

use crate::entities::{
    alerts, incidents, playbook_runs, playbooks, postmortems, run_steps, timeline_events,
};
use crate::types::{
    normalize_severity, normalize_status, now_ms_i64, now_ms_f64, AlertInput, AlertView,
    IncidentView, PlaybookDef, PlaybookStep, PlaybookView, PostmortemView, RunStepView, RunView,
    TimelineEventView,
};

pub async fn init_tables(db: &DatabaseConnection) -> Result<(), String> {
    run_sql(
        db,
        "CREATE TABLE IF NOT EXISTS incidents (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            correlation_key TEXT NOT NULL,
            status          TEXT NOT NULL DEFAULT 'open',
            severity        TEXT NOT NULL DEFAULT 'medium',
            title           TEXT NOT NULL DEFAULT '',
            summary         TEXT NOT NULL DEFAULT '',
            source          TEXT NOT NULL DEFAULT '',
            entity          TEXT NOT NULL DEFAULT '',
            alert_count     INTEGER NOT NULL DEFAULT 0,
            first_seen_ms   REAL NOT NULL,
            last_seen_ms    REAL NOT NULL,
            assignee        TEXT NOT NULL DEFAULT '',
            created_at      INTEGER NOT NULL,
            updated_at      INTEGER NOT NULL
        );",
    )
    .await?;

    run_sql(
        db,
        "CREATE TABLE IF NOT EXISTS alerts (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            incident_id     INTEGER NOT NULL DEFAULT 0,
            source          TEXT NOT NULL DEFAULT '',
            kind            TEXT NOT NULL DEFAULT '',
            severity        TEXT NOT NULL DEFAULT 'medium',
            entity          TEXT NOT NULL DEFAULT '',
            detail          TEXT NOT NULL DEFAULT '{}',
            correlation_key TEXT NOT NULL DEFAULT '',
            received_at     INTEGER NOT NULL
        );",
    )
    .await?;

    run_sql(
        db,
        "CREATE TABLE IF NOT EXISTS playbooks (
            id             INTEGER PRIMARY KEY AUTOINCREMENT,
            name           TEXT NOT NULL DEFAULT '',
            description    TEXT NOT NULL DEFAULT '',
            auto_severity  TEXT NOT NULL DEFAULT '',
            steps          TEXT NOT NULL DEFAULT '[]',
            enabled        INTEGER NOT NULL DEFAULT 1,
            created_at     INTEGER NOT NULL
        );",
    )
    .await?;

    run_sql(
        db,
        "CREATE TABLE IF NOT EXISTS playbook_runs (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            playbook_id INTEGER NOT NULL DEFAULT 0,
            incident_id INTEGER NOT NULL,
            status      TEXT NOT NULL DEFAULT 'pending',
            initiator   TEXT NOT NULL DEFAULT 'auto',
            started_at  INTEGER NOT NULL,
            finished_at INTEGER NOT NULL DEFAULT 0
        );",
    )
    .await?;

    run_sql(
        db,
        "CREATE TABLE IF NOT EXISTS run_steps (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            run_id      INTEGER NOT NULL,
            seq         INTEGER NOT NULL DEFAULT 0,
            name        TEXT NOT NULL DEFAULT '',
            status      TEXT NOT NULL DEFAULT 'pending',
            gate        INTEGER NOT NULL DEFAULT 0,
            gate_status TEXT NOT NULL DEFAULT 'none',
            output      TEXT NOT NULL DEFAULT '',
            started_at  INTEGER NOT NULL DEFAULT 0,
            finished_at INTEGER NOT NULL DEFAULT 0
        );",
    )
    .await?;

    run_sql(
        db,
        "CREATE TABLE IF NOT EXISTS timeline_events (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            incident_id  INTEGER NOT NULL,
            run_id       INTEGER NOT NULL DEFAULT 0,
            actor        TEXT NOT NULL DEFAULT 'system',
            kind         TEXT NOT NULL DEFAULT 'note',
            payload      TEXT NOT NULL DEFAULT '{}',
            ts_ms        REAL NOT NULL
        );",
    )
    .await?;

    run_sql(
        db,
        "CREATE TABLE IF NOT EXISTS postmortems (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            incident_id  INTEGER NOT NULL,
            content      TEXT NOT NULL DEFAULT '',
            created_at   INTEGER NOT NULL
        );",
    )
    .await?;

    Ok(())
}

async fn run_sql(db: &DatabaseConnection, sql: &str) -> Result<(), String> {
    db.execute(sea_orm::Statement::from_string(db.get_database_backend(), sql.to_string()))
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Cheap liveness check of the underlying connection.
pub async fn ping(db: &DatabaseConnection) -> Result<(), String> {
    run_sql(db, "SELECT 1").await
}

// ---------------------------------------------------------------------------
// Incidents
// ---------------------------------------------------------------------------

fn to_incident(row: incidents::Model) -> IncidentView {
    IncidentView {
        id: row.id as i64,
        correlation_key: row.correlation_key,
        status: row.status,
        severity: row.severity,
        title: row.title,
        summary: row.summary,
        source: row.source,
        entity: row.entity,
        alert_count: row.alert_count,
        first_seen_ms: row.first_seen_ms,
        last_seen_ms: row.last_seen_ms,
        assignee: row.assignee,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }
}

/// Create a new incident from its first alert.
pub async fn insert_incident(
    db: &DatabaseConnection,
    key: &str,
    alert: &AlertInput,
) -> Result<i64, String> {
    let now = now_ms_i64();
    let ts = if alert.ts_ms > 0.0 { alert.ts_ms } else { now_ms_f64() };
    let entity = alert.entity.clone();
    let title = format!("[{}] {}", alert.source.to_uppercase(), entity);
    let row = incidents::ActiveModel {
        id: NotSet,
        correlation_key: Set(key.to_string()),
        status: Set("open".to_string()),
        severity: Set(normalize_severity(&alert.severity)),
        title: Set(title),
        summary: Set(String::new()),
        source: Set(alert.source.clone()),
        entity: Set(entity),
        alert_count: Set(1),
        first_seen_ms: Set(ts),
        last_seen_ms: Set(ts),
        assignee: Set(String::new()),
        created_at: Set(now),
        updated_at: Set(now),
    };

    let inserted = row.insert(db).await.map_err(|e| e.to_string())?;
    Ok(inserted.id as i64)
}

/// Find the most-recently-seen open incident matching a correlation key.
pub async fn find_open_incident(
    db: &DatabaseConnection,
    key: &str,
) -> Result<Option<IncidentView>, String> {
    let row = incidents::Entity::find()
        .filter(incidents::Column::CorrelationKey.eq(key))
        .filter(incidents::Column::Status.ne("closed"))
        .filter(incidents::Column::Status.ne("resolved"))
        .order_by_desc(incidents::Column::LastSeenMs)
        .one(db)
        .await
        .map_err(|e| e.to_string())?;
    Ok(row.map(to_incident))
}

/// Attach an alert to an incident: bump counters and refresh severity/time.
pub async fn bump_incident(
    db: &DatabaseConnection,
    incident_id: i64,
    severity: &str,
    ts: f64,
) -> Result<IncidentView, String> {
    let model = incidents::Entity::find_by_id(incident_id as i32)
        .one(db)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "incident_not_found".to_string())?;

    let merged_sev = {
        let a = crate::types::severity_rank(&model.severity);
        let b = crate::types::severity_rank(severity);
        if b > a {
            normalize_severity(severity)
        } else {
            model.severity.clone()
        }
    };
    let new_count = model.alert_count + 1;

    let mut active: incidents::ActiveModel = model.into();
    active.alert_count = Set(new_count);
    active.severity = Set(merged_sev);
    active.last_seen_ms = Set(ts);
    active.updated_at = Set(now_ms_i64());
    let row = active.update(db).await.map_err(|e| e.to_string())?;
    Ok(to_incident(row))
}

/// List incidents newest-first, with optional status/severity filters.
pub async fn list_incidents(
    db: &DatabaseConnection,
    limit: u64,
    status: Option<&str>,
    severity: Option<&str>,
) -> Result<Vec<IncidentView>, String> {
    let mut q = incidents::Entity::find().order_by_desc(incidents::Column::Id);
    if let Some(s) = status {
        q = q.filter(incidents::Column::Status.eq(s));
    }
    if let Some(s) = severity {
        q = q.filter(incidents::Column::Severity.eq(normalize_severity(s)));
    }
    let rows = q.limit(limit).all(db).await.map_err(|e| e.to_string())?;
    Ok(rows.into_iter().map(to_incident).collect())
}

pub async fn find_incident(db: &DatabaseConnection, id: i64) -> Result<Option<IncidentView>, String> {
    let row = incidents::Entity::find_by_id(id as i32)
        .one(db)
        .await
        .map_err(|e| e.to_string())?;
    Ok(row.map(to_incident))
}

/// Apply a lifecycle transition, returning the updated view.
pub async fn update_incident_status(
    db: &DatabaseConnection,
    id: i64,
    status: &str,
) -> Result<IncidentView, String> {
    let model = incidents::Entity::find_by_id(id as i32)
        .one(db)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "incident_not_found".to_string())?;
    let mut active: incidents::ActiveModel = model.into();
    active.status = Set(normalize_status(status));
    active.updated_at = Set(now_ms_i64());
    let row = active.update(db).await.map_err(|e| e.to_string())?;
    Ok(to_incident(row))
}

pub async fn assign_incident(
    db: &DatabaseConnection,
    id: i64,
    assignee: &str,
) -> Result<IncidentView, String> {
    let model = incidents::Entity::find_by_id(id as i32)
        .one(db)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "incident_not_found".to_string())?;
    let mut active: incidents::ActiveModel = model.into();
    active.assignee = Set(assignee.to_string());
    active.updated_at = Set(now_ms_i64());
    let row = active.update(db).await.map_err(|e| e.to_string())?;
    Ok(to_incident(row))
}

// ---------------------------------------------------------------------------
// Alerts
// ---------------------------------------------------------------------------

fn to_alert(row: alerts::Model) -> AlertView {
    AlertView {
        id: row.id as i64,
        incident_id: row.incident_id,
        source: row.source,
        kind: row.kind,
        severity: row.severity,
        entity: row.entity,
        detail: serde_json::from_str(&row.detail).unwrap_or(serde_json::json!({})),
        correlation_key: row.correlation_key,
        received_at: row.received_at,
    }
}

/// Persist an alert (unattached). Returns the row id.
pub async fn insert_alert(db: &DatabaseConnection, alert: &AlertInput) -> Result<i64, String> {
    let row = alerts::ActiveModel {
        id: NotSet,
        incident_id: Set(0),
        source: Set(alert.source.clone()),
        kind: Set(alert.kind.clone()),
        severity: Set(normalize_severity(&alert.severity)),
        entity: Set(alert.entity.clone()),
        detail: Set(alert.detail.to_string()),
        correlation_key: Set("".to_string()),
        received_at: Set(now_ms_i64()),
    };

    let inserted = row.insert(db).await.map_err(|e| e.to_string())?;
    Ok(inserted.id as i64)
}

/// Link a stored alert to an incident and record its correlation key.
pub async fn attach_alert(
    db: &DatabaseConnection,
    alert_id: i64,
    incident_id: i64,
    key: &str,
) -> Result<(), String> {
    let model = alerts::Entity::find_by_id(alert_id as i32)
        .one(db)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "alert_not_found".to_string())?;
    let mut active: alerts::ActiveModel = model.into();
    active.incident_id = Set(incident_id);
    active.correlation_key = Set(key.to_string());
    active.update(db).await.map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn list_incident_alerts(
    db: &DatabaseConnection,
    incident_id: i64,
) -> Result<Vec<AlertView>, String> {
    let rows = alerts::Entity::find()
        .filter(alerts::Column::IncidentId.eq(incident_id))
        .order_by_desc(alerts::Column::Id)
        .all(db)
        .await
        .map_err(|e| e.to_string())?;
    Ok(rows.into_iter().map(to_alert).collect())
}

// ---------------------------------------------------------------------------
// Timeline
// ---------------------------------------------------------------------------

fn to_timeline(row: timeline_events::Model) -> TimelineEventView {
    TimelineEventView {
        id: row.id as i64,
        incident_id: row.incident_id,
        run_id: row.run_id,
        actor: row.actor,
        kind: row.kind,
        payload: serde_json::from_str(&row.payload).unwrap_or(serde_json::json!({})),
        ts_ms: row.ts_ms,
    }
}

/// Append a timeline entry and return its id.
pub async fn push_timeline(
    db: &DatabaseConnection,
    incident_id: i64,
    run_id: i64,
    actor: &str,
    kind: &str,
    payload: serde_json::Value,
) -> Result<i64, String> {
    let row = timeline_events::ActiveModel {
        id: NotSet,
        incident_id: Set(incident_id),
        run_id: Set(run_id),
        actor: Set(actor.to_string()),
        kind: Set(kind.to_string()),
        payload: Set(payload.to_string()),
        ts_ms: Set(now_ms_f64()),
    };
    let inserted = row.insert(db).await.map_err(|e| e.to_string())?;
    Ok(inserted.id as i64)
}

pub async fn list_timeline(
    db: &DatabaseConnection,
    incident_id: i64,
) -> Result<Vec<TimelineEventView>, String> {
    let rows = timeline_events::Entity::find()
        .filter(timeline_events::Column::IncidentId.eq(incident_id))
        .order_by_asc(timeline_events::Column::Id)
        .all(db)
        .await
        .map_err(|e| e.to_string())?;
    Ok(rows.into_iter().map(to_timeline).collect())
}

// ---------------------------------------------------------------------------
// Playbooks
// ---------------------------------------------------------------------------

fn to_playbook(row: playbooks::Model) -> PlaybookView {
    let def: PlaybookDef = serde_json::from_str(&row.steps).unwrap_or(PlaybookDef {
        name: row.name.clone(),
        description: row.description.clone(),
        auto_severity: row.auto_severity.clone(),
        steps: Vec::new(),
    });
    PlaybookView {
        id: row.id as i64,
        name: def.name,
        description: def.description,
        auto_severity: def.auto_severity,
        enabled: row.enabled != 0,
        steps: def.steps,
        created_at: row.created_at,
    }
}

/// Persist a playbook definition. `definition_json` is the canonical
/// `PlaybookDef` JSON; the playbook row extracts name/severity for filtering.
pub async fn insert_playbook(
    db: &DatabaseConnection,
    def: &PlaybookDef,
) -> Result<i64, String> {
    let row = playbooks::ActiveModel {
        id: NotSet,
        name: Set(def.name.clone()),
        description: Set(def.description.clone()),
        auto_severity: Set(normalize_severity(&def.auto_severity)),
        steps: Set(def.canonical().to_string()),
        enabled: Set(1),
        created_at: Set(now_ms_i64()),
    };
    let inserted = row.insert(db).await.map_err(|e| e.to_string())?;
    Ok(inserted.id as i64)
}

pub async fn list_playbooks(db: &DatabaseConnection) -> Result<Vec<PlaybookView>, String> {
    let rows = playbooks::Entity::find()
        .order_by_desc(playbooks::Column::Id)
        .all(db)
        .await
        .map_err(|e| e.to_string())?;
    Ok(rows.into_iter().map(to_playbook).collect())
}

pub async fn find_playbook(db: &DatabaseConnection, id: i64) -> Result<Option<PlaybookView>, String> {
    let row = playbooks::Entity::find_by_id(id as i32)
        .one(db)
        .await
        .map_err(|e| e.to_string())?;
    Ok(row.map(to_playbook))
}

/// First `enabled` playbook whose auto_severity threshold the incident meets
/// (ranked). Ties broken by most recently created.
pub async fn pick_auto_playbook(
    db: &DatabaseConnection,
    incident_severity: &str,
) -> Result<Option<PlaybookView>, String> {
    let sev = crate::types::severity_rank(incident_severity);
    let rows = playbooks::Entity::find()
        .filter(playbooks::Column::Enabled.eq(1))
        .order_by_asc(playbooks::Column::Id)
        .all(db)
        .await
        .map_err(|e| e.to_string())?;

    Ok(rows
        .into_iter()
        .map(to_playbook)
        .find(|p| {
            !p.auto_severity.is_empty()
                && crate::types::severity_rank(&p.auto_severity) <= sev
        }))
}

// ---------------------------------------------------------------------------
// Runs
// ---------------------------------------------------------------------------

fn to_run(row: playbook_runs::Model) -> RunView {
    RunView {
        id: row.id as i64,
        playbook_id: row.playbook_id,
        incident_id: row.incident_id,
        status: row.status,
        initiator: row.initiator,
        started_at: row.started_at,
        finished_at: row.finished_at,
    }
}

pub async fn insert_run(
    db: &DatabaseConnection,
    playbook_id: i64,
    incident_id: i64,
    initiator: &str,
) -> Result<i64, String> {
    let row = playbook_runs::ActiveModel {
        id: NotSet,
        playbook_id: Set(playbook_id),
        incident_id: Set(incident_id),
        status: Set("pending".to_string()),
        initiator: Set(initiator.to_string()),
        started_at: Set(now_ms_i64()),
        finished_at: Set(0),
    };
    let inserted = row.insert(db).await.map_err(|e| e.to_string())?;
    Ok(inserted.id as i64)
}

pub async fn set_run_status(
    db: &DatabaseConnection,
    run_id: i64,
    status: &str,
) -> Result<(), String> {
    let model = playbook_runs::Entity::find_by_id(run_id as i32)
        .one(db)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "run_not_found".to_string())?;
    let normalized = crate::types::normalize_run_status(status);
    let terminal = matches!(normalized.as_str(), "succeeded" | "failed" | "cancelled");

    let mut active: playbook_runs::ActiveModel = model.into();
    active.status = Set(normalized);
    if terminal {
        active.finished_at = Set(now_ms_i64());
    }
    active.update(db).await.map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn find_run(db: &DatabaseConnection, run_id: i64) -> Result<Option<RunView>, String> {
    let row = playbook_runs::Entity::find_by_id(run_id as i32)
        .one(db)
        .await
        .map_err(|e| e.to_string())?;
    Ok(row.map(to_run))
}

pub async fn list_incident_runs(
    db: &DatabaseConnection,
    incident_id: i64,
) -> Result<Vec<RunView>, String> {
    let rows = playbook_runs::Entity::find()
        .filter(playbook_runs::Column::IncidentId.eq(incident_id))
        .order_by_desc(playbook_runs::Column::Id)
        .all(db)
        .await
        .map_err(|e| e.to_string())?;
    Ok(rows.into_iter().map(to_run).collect())
}

// ---------------------------------------------------------------------------
// Run steps
// ---------------------------------------------------------------------------

fn to_run_step(row: run_steps::Model) -> RunStepView {
    RunStepView {
        id: row.id as i64,
        run_id: row.run_id,
        seq: row.seq,
        name: row.name,
        status: row.status,
        gate: row.gate != 0,
        gate_status: row.gate_status,
        output: serde_json::from_str(&row.output).unwrap_or(serde_json::json!({})),
        started_at: row.started_at,
        finished_at: row.finished_at,
    }
}

/// Materialise a run's steps from the playbook definition. Returns the list.
pub async fn create_run_steps(
    db: &DatabaseConnection,
    run_id: i64,
    steps: &[PlaybookStep],
) -> Result<Vec<RunStepView>, String> {
    let mut out = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        let row = run_steps::ActiveModel {
            id: NotSet,
            run_id: Set(run_id),
            seq: Set(i as i64),
            name: Set(step.name.clone()),
            status: Set("pending".to_string()),
            gate: Set(if step.gate { 1 } else { 0 }),
            gate_status: Set(if step.gate { "open".to_string() } else { "none".to_string() }),
            output: Set("".to_string()),
            started_at: Set(0),
            finished_at: Set(0),
        };
        let inserted = row.insert(db).await.map_err(|e| e.to_string())?;
        out.push(to_run_step(inserted));
    }
    Ok(out)
}

pub async fn list_run_steps(
    db: &DatabaseConnection,
    run_id: i64,
) -> Result<Vec<RunStepView>, String> {
    let rows = run_steps::Entity::find()
        .filter(run_steps::Column::RunId.eq(run_id))
        .order_by_asc(run_steps::Column::Seq)
        .all(db)
        .await
        .map_err(|e| e.to_string())?;
    Ok(rows.into_iter().map(to_run_step).collect())
}

#[derive(Clone, Copy)]
pub enum StepGateStatus {
    Approve,
    Reject,
}

/// Record an analyst approval/rejection of a gated step.
pub async fn set_step_gate(
    db: &DatabaseConnection,
    run_id: i64,
    seq: i64,
    decision: StepGateStatus,
) -> Result<(), String> {
    let model = run_steps::Entity::find()
        .filter(run_steps::Column::RunId.eq(run_id))
        .filter(run_steps::Column::Seq.eq(seq))
        .one(db)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "step_not_found".to_string())?;

    let mut active: run_steps::ActiveModel = model.into();
    active.gate_status = Set(match decision {
        StepGateStatus::Approve => "approved".to_string(),
        StepGateStatus::Reject => "rejected".to_string(),
    });
    active.update(db).await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Mark a step started when the engine picks it up.
pub async fn start_step(db: &DatabaseConnection, step_id: i64) -> Result<(), String> {
    let model = run_steps::Entity::find_by_id(step_id as i32)
        .one(db)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "step_not_found".to_string())?;
    let mut active: run_steps::ActiveModel = model.into();
    active.status = Set("running".to_string());
    active.started_at = Set(now_ms_i64());
    active.update(db).await.map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn finish_step(
    db: &DatabaseConnection,
    step_id: i64,
    ok: bool,
    output: serde_json::Value,
) -> Result<(), String> {
    let model = run_steps::Entity::find_by_id(step_id as i32)
        .one(db)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "step_not_found".to_string())?;
    let mut active: run_steps::ActiveModel = model.into();
    active.status = Set(if ok { "succeeded".to_string() } else { "failed".to_string() });
    active.output = Set(output.to_string());
    active.finished_at = Set(now_ms_i64());
    active.update(db).await.map_err(|e| e.to_string())?;
    Ok(())
}

/// Skip a step (gate rejected). Non-destructive no-op for later steps.
pub async fn cancel_pending_steps(
    db: &DatabaseConnection,
    run_id: i64,
    after_seq: i64,
) -> Result<(), String> {
    let steps = run_steps::Entity::find()
        .filter(run_steps::Column::RunId.eq(run_id))
        .filter(run_steps::Column::Seq.gt(after_seq))
        .filter(run_steps::Column::Status.eq("pending"))
        .all(db)
        .await
        .map_err(|e| e.to_string())?;
    for step in steps {
        let mut active: run_steps::ActiveModel = step.into();
        active.status = Set("cancelled".to_string());
        active.gate_status = Set("none".to_string());
        active.update(db).await.map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Postmortems
// ---------------------------------------------------------------------------

fn to_postmortem(row: postmortems::Model) -> PostmortemView {
    PostmortemView {
        id: row.id as i64,
        incident_id: row.incident_id,
        content: row.content,
        created_at: row.created_at,
    }
}

pub async fn insert_postmortem(
    db: &DatabaseConnection,
    incident_id: i64,
    content: &str,
) -> Result<i64, String> {
    let row = postmortems::ActiveModel {
        id: NotSet,
        incident_id: Set(incident_id),
        content: Set(content.to_string()),
        created_at: Set(now_ms_i64()),
    };
    let inserted = row.insert(db).await.map_err(|e| e.to_string())?;
    Ok(inserted.id as i64)
}

pub async fn latest_postmortem(
    db: &DatabaseConnection,
    incident_id: i64,
) -> Result<Option<PostmortemView>, String> {
    let top = postmortems::Entity::find()
        .filter(postmortems::Column::IncidentId.eq(incident_id))
        .order_by_desc(postmortems::Column::Id)
        .one(db)
        .await
        .map_err(|e| e.to_string())?;
    Ok(top.map(to_postmortem))
}