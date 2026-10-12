use std::sync::Arc;

use serde_json::json;

use crate::db;
use crate::state::AppState;
use crate::types::{normalize_severity, now_ms_f64, AlertInput, AlertView, IncidentView};

/// Outcome of correlating one alert.
pub struct IngestOutcome {
    pub incident: IncidentView,
    pub alert: AlertView,
    /// `true` when this alert created a brand-new incident.
    pub created_incident: bool,
}

/// Compute the correlation key that binds alerts to an incident. An explicit
/// `correlation_key` wins; otherwise `source:kind:entity` is used, which is the
/// practical fingerprint for CyberFlow/LuxShield/ACC alerts.
pub fn correlation_key(alert: &AlertInput) -> String {
    if let Some(key) = &alert.correlation_key {
        let trimmed = key.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let entity = if alert.entity.trim().is_empty() {
        "unknown"
    } else {
        alert.entity.trim()
    };
    let kind = if alert.kind.trim().is_empty() {
        "generic"
    } else {
        alert.kind.trim()
    };
    format!("{}:{}:{}", alert.source.trim().to_ascii_lowercase(), kind.to_ascii_lowercase(), entity)
}

/// Correlate an incoming alert: persist it, attach it to an existing open
/// incident when one matches the key inside the dedup window, otherwise open a
/// new incident. Timeline entries and SSE frames are emitted along the way.
pub async fn ingest_alert(st: &Arc<AppState>, alert: AlertInput) -> Result<IngestOutcome, String> {
    let db = st.db.as_ref();
    let key = correlation_key(&alert);
    let ts = if alert.ts_ms > 0.0 { alert.ts_ms } else { now_ms_f64() };

    // 1. Persist the raw alert first so nothing is lost.
    let alert_id = db::insert_alert(db, &alert).await?;

    // 2. Look for an open incident within the dedup window.
    let existing = db::find_open_incident(db, &key).await?;
    let within_window = existing
        .as_ref()
        .map(|inc| (ts - inc.last_seen_ms).abs() <= st.dedup_window_ms as f64)
        .unwrap_or(false);

    let (incident, created_incident) = if let (Some(inc), true) = (existing, within_window) {
        // Merge: bump counters/severity, refresh last-seen.
        let updated = db::bump_incident(db, inc.id, &alert.severity, ts).await?;
        db::push_timeline(
            db,
            updated.id,
            0,
            "system",
            "alert_merged",
            json!({
                "alert_id": alert_id,
                "source": alert.source,
                "kind": alert.kind,
                "severity": normalize_severity(&alert.severity),
            }),
        )
        .await?;
        st.feed
            .publish("alert", json!({ "incident_id": updated.id, "id": alert_id, "source": alert.source }))
            .await;
        st.feed.publish("incident", json!(updated)).await;
        (updated, false)
    } else {
        // New incident keyed off this alert.
        let incident_id = db::insert_incident(db, &key, &alert).await?;
        db::push_timeline(
            db,
            incident_id,
            0,
            "system",
            "incident_opened",
            json!({
                "alert_id": alert_id,
                "source": alert.source,
                "kind": alert.kind,
                "severity": normalize_severity(&alert.severity),
                "correlation_key": key,
            }),
        )
        .await?;
        let incident = db::find_incident(db, incident_id)
            .await?
            .ok_or_else(|| "incident_not_found_after_insert".to_string())?;
        st.feed.publish("incident", json!(incident)).await;
        (incident, true)
    };

    db::attach_alert(db, alert_id, incident.id, &key).await?;

    let alert_view = AlertView {
        id: alert_id,
        incident_id: incident.id,
        source: alert.source.clone(),
        kind: alert.kind.clone(),
        severity: normalize_severity(&alert.severity),
        entity: alert.entity.clone(),
        detail: alert.detail.clone(),
        correlation_key: key,
        received_at: crate::types::now_ms_i64(),
    };

    Ok(IngestOutcome {
        incident,
        alert: alert_view,
        created_incident,
    })
}

/// Convenience: manually add a timeline note for an incident.
pub async fn add_note(
    st: &Arc<AppState>,
    incident_id: i64,
    actor: &str,
    text: &str,
) -> Result<i64, String> {
    let id = db::push_timeline(
        st.db.as_ref(),
        incident_id,
        0,
        actor,
        "note",
        json!({ "text": text }),
    )
    .await?;
    st.feed
        .publish(
            "timeline",
            json!({ "incident_id": incident_id, "id": id, "actor": actor, "kind": "note", "text": text }),
        )
        .await;
    Ok(id)
}