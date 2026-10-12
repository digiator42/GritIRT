//! SSE client: bridges GritIRT's live incident feed into reactive state.
//!
//! Wraps `web_sys::EventSource` — the browser handles automatic reconnection
//! backoff natively. Named events map 1:1 to object types: `incident`
//! (create/update), `alert`, `timeline`, `run`, `step`. The queue is patched
//! in place; the selected dossier re-fetches on relevant frames.

use std::cell::RefCell;

use gloo_timers::future::TimeoutFuture;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use web_sys::EventSource;

use crate::api;
use crate::state::{Alert, AppState, ConnStatus, Incident, TimelineEvent, TOAST_KEEP, TOAST_TTL_MS};

thread_local! {
    /// The active stream, if any. `EventSource` is not `Clone`, so it lives
    /// behind a `RefCell` rather than inside `AppState`.
    static SOURCE: RefCell<Option<EventSource>> = const { RefCell::new(None) };
}

/// Named SSE events the backend emits.
const KINDS: [&str; 5] = ["incident", "alert", "timeline", "run", "step"];

/// Open the stream (closing any previous one first). Idempotent.
pub fn connect(app: &AppState) {
    disconnect();
    app.conn.set(ConnStatus::Connecting);

    let url = api::stream_url();
    let source = match EventSource::new(&url) {
        Ok(s) => s,
        Err(err) => {
            web_sys::console::error_1(&format!("SSE open failed: {err:?}").into());
            app.conn.set(ConnStatus::Closed);
            return;
        }
    };

    let on_open = {
        let conn = app.conn;
        Closure::wrap(Box::new(move || conn.set(ConnStatus::Open)) as Box<dyn FnMut()>)
    };
    source.set_onopen(Some(on_open.as_ref().unchecked_ref()));
    on_open.forget();

    // EventSource hides the HTTP status, so probe /health once: forbidden
    // means bad key or a banned viewer.
    let on_error = {
        let conn = app.conn;
        let app = app.clone();
        Closure::wrap(Box::new(move || {
            conn.set(ConnStatus::Reconnecting);
            wasm_bindgen_futures::spawn_local(async move {
                match api::get_health().await {
                    Ok(_) => app.banned.set(false),
                    Err(e) if e.is_forbidden() => app.banned.set(true),
                    Err(_) => {}
                }
            });
        }) as Box<dyn FnMut()>)
    };
    source.set_onerror(Some(on_error.as_ref().unchecked_ref()));
    on_error.forget();

    for kind in KINDS {
        let on_event = {
            let app = app.clone();
            Closure::wrap(Box::new(move |event: web_sys::MessageEvent| {
                let Some(raw) = event.data().as_string() else {
                    return;
                };
                match kind {
                    "incident" => on_incident(&app, &raw),
                    "alert" => on_alert(&app, &raw),
                    "timeline" => on_timeline(&app, &raw),
                    "run" => on_run_frame(&app, &raw),
                    "step" => on_step_frame(&app, &raw),
                    _ => {}
                }
            }) as Box<dyn FnMut(web_sys::MessageEvent)>)
        };

        if let Err(err) =
            source.add_event_listener_with_callback(kind, on_event.as_ref().unchecked_ref())
        {
            web_sys::console::error_1(
                &format!("failed to register '{kind}' listener: {err:?}").into(),
            );
        }
        on_event.forget();
    }

    SOURCE.with(|slot| *slot.borrow_mut() = Some(source));
}

/// Close the stream (no-op when already closed).
pub fn disconnect() {
    SOURCE.with(|slot| {
        if let Some(source) = slot.borrow_mut().take() {
            source.close();
        }
    });
}

fn warn_skip(err: impl std::fmt::Display) {
    web_sys::console::warn_1(&format!("skipping malformed SSE event: {err}").into());
}

/// `incident` frame carries the full IncidentView: upsert + toast on new.
fn on_incident(app: &AppState, raw: &str) {
    let inc: Incident = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => return warn_skip(e),
    };
    let is_new = !app.incidents.get().iter().any(|e| e.id == inc.id);
    app.upsert_incident(inc.clone());
    if is_new {
        push_toast(
            app,
            format!("{} {}", inc.sev().as_str(), inc.title),
            format!("{} · {} alerts", inc.entity, inc.alert_count),
            matches!(
                crate::state::Severity::parse(&inc.severity),
                crate::state::Severity::High | crate::state::Severity::Critical
            ),
        );
    }
    // Keep an open dossier fresh.
    if app.selected_id.get() == Some(inc.id) {
        refresh_dossier(app, inc.id);
    }
}

/// `alert` frame carries the AlertView.
fn on_alert(app: &AppState, raw: &str) {
    let alert: Alert = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => return warn_skip(e),
    };
    if app.selected_id.get() == Some(alert.incident_id) {
        refresh_dossier(app, alert.incident_id);
    }
}

/// `timeline` frame carries the TimelineEventView.
fn on_timeline(app: &AppState, raw: &str) {
    let ev: TimelineEvent = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => return warn_skip(e),
    };
    if app.selected_id.get() == Some(ev.incident_id) {
        refresh_dossier(app, ev.incident_id);
    }
}

#[derive(serde::Deserialize)]
struct RunFrame {
    incident_id: i64,
}

/// `run` / `step` frames carry RunView / RunStepView (both have incident_id).
fn on_run_frame(app: &AppState, raw: &str) {
    let frame: RunFrame = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => return warn_skip(e),
    };
    if app.selected_id.get() == Some(frame.incident_id) {
        refresh_dossier(app, frame.incident_id);
    }
}

fn on_step_frame(app: &AppState, raw: &str) {
    on_run_frame(app, raw);
}

/// Re-fetch the selected incident's dossier (alerts + timeline + runs).
pub fn refresh_dossier(app: &AppState, id: i64) {
    let app = app.clone();
    wasm_bindgen_futures::spawn_local(async move {
        match api::get_dossier(id).await {
            Ok(d) => {
                app.banned.set(false);
                app.api_error.set(String::new());
                app.upsert_incident(d.incident);
                app.dossier_alerts.with_mut(|v| *v = d.alerts);
                app.dossier_timeline.with_mut(|v| *v = d.timeline);
                app.dossier_runs.with_mut(|v| *v = d.runs);
            }
            Err(e) => {
                if e.is_forbidden() {
                    app.banned.set(true);
                }
                app.api_error.set(e.to_string());
            }
        }
    });
}

/// Initial queue sync (first paint + reconnect).
pub fn sync_incidents(app: &AppState) {
    let app = app.clone();
    wasm_bindgen_futures::spawn_local(async move {
        match api::list_incidents(100, "", "").await {
            Ok((count, incidents)) => {
                app.banned.set(false);
                app.api_error.set(String::new());
                app.metrics.incident_total.set(count);
                for inc in incidents {
                    app.upsert_incident(inc);
                }
            }
            Err(e) => {
                if e.is_forbidden() {
                    app.banned.set(true);
                }
                app.api_error.set(e.to_string());
            }
        }
    });
}

fn push_toast(app: &AppState, title: String, detail: String, critical: bool) {
    let id = app.next_toast_id();
    app.toasts.with_mut(|v| {
        v.push(crate::state::Toast {
            id,
            title,
            detail,
            critical,
        });
        while v.len() > TOAST_KEEP {
            v.remove(0);
        }
    });

    let toasts = app.toasts.clone();
    wasm_bindgen_futures::spawn_local(async move {
        TimeoutFuture::new(TOAST_TTL_MS).await;
        toasts.with_mut(|v| {
            if let Some(pos) = v.iter().position(|t| t.id == id) {
                v.remove(pos);
            }
        });
    });
}
