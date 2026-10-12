//! GritIRT console — incident response orchestration plane.
//!
//! Watches CyberFlow / LuxShield / ACC alerts as they correlate into
//! incidents, and drives playbook runs (approval gates included) to closure:
//!
//! - SSE incident/alert/timeline/run/step feed ([`net`]) into a
//!   severity-sorted queue with a per-incident dossier
//! - Dossier: status/assignee actions, linked alerts, runs with approve/reject
//!   gates, notes composer, append-only timeline, postmortem builder
//! - Playbook panel: runbook list, JSON create form, one-click runs

use veloasm::prelude::*;
use wasm_bindgen::prelude::wasm_bindgen;

mod api;
mod components;
mod net;
mod state;

use crate::components::*;
use crate::state::AppState;

#[wasm_bindgen(start)]
pub fn main() {
    console_error_panic_hook::set_once();
    run_app();
}

pub fn run_app() {
    // Seed global context before mounting so every component can pull the
    // shared state with `use_context::<AppState>()`.
    let app = AppState::new();
    provide!(app.clone());

    // Open the SSE stream, sync the queue, and poll health/connectors.
    net::connect(&app);
    net::sync_incidents(&app);
    poll_health(&app);

    let shell = view! {
        <div class="app-shell min-h-screen flex flex-col bg-ink text-slate-200 text-sm">
            <TopBar />
            <BanBanner />
            <MetricsBar />

            <main class="grid gap-4 p-4 flex-1 min-h-0 lg:grid-cols-[380px_minmax(0,1fr)_340px]">
                <div class="flex flex-col gap-4 min-w-0">
                    <IncidentQueue />
                </div>
                <div class="flex flex-col gap-4 min-w-0">
                    <IncidentDetail />
                </div>
                <aside class="flex flex-col gap-4 min-w-0">
                    <PlaybookPanel />
                </aside>
            </main>

            <ToastStack />
        </div>
    };
    mount(shell);
}

/// Poll `/health` for service state and connector wiring.
fn poll_health(app: &AppState) {
    let refresh = {
        let app = app.clone();
        move || {
            let app = app.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match api::get_health().await {
                    Ok(h) => {
                        app.banned.set(false);
                        app.api_error.set(String::new());
                        let conns = if h.connectors.is_empty() {
                            "no connectors".to_string()
                        } else {
                            h.connectors.join(" · ")
                        };
                        app.connectors.set(conns);
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
    };

    refresh();
    let tick = refresh.clone();
    let _interval = gloo_timers::callback::Interval::new(15_000, move || tick());
    std::mem::forget(_interval);
}
