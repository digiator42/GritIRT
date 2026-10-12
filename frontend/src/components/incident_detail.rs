//! Incident detail: dossier header with status/assignee actions, linked
//! alerts, run list with approval gates + step expansion, notes composer,
//! timeline audit, and the postmortem builder.

use veloasm::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::Event;

use crate::state::{
    format_date_time, format_time, short_json, AppState, Run, RunStep, Severity, STATUSES,
};

fn status_pill(status: &str) -> &'static str {
    match status {
        "open" => "bg-sky-500/15 text-sky-300",
        "triaging" => "bg-cyan-500/15 text-cyan-300",
        "investigating" => "bg-violet-500/15 text-violet-300",
        "contained" => "bg-amber-500/15 text-amber-300",
        "eradicating" => "bg-orange-500/15 text-orange-300",
        "recovering" => "bg-teal-500/15 text-teal-300",
        "resolved" => "bg-emerald-500/15 text-emerald-300",
        "closed" => "bg-slate-500/15 text-slate-400",
        _ => "bg-slate-500/15 text-slate-300",
    }
}

fn run_pill(status: &str) -> &'static str {
    match status {
        "succeeded" => "bg-emerald-500/15 text-emerald-300",
        "failed" => "bg-rose-500/15 text-rose-300",
        "running" | "pending" => "bg-sky-500/15 text-sky-300",
        "waiting_approval" => "bg-amber-500/15 text-amber-300 blink",
        "cancelled" => "bg-slate-500/15 text-slate-400",
        _ => "bg-slate-500/15 text-slate-300",
    }
}

#[component]
pub fn IncidentDetail() -> DomNode {
    let app = use_context::<AppState>().expect("AppState must be provided before mounting");
    let selected = app.selected_id;

    view! {
        <section class="bg-panel border border-edge rounded-lg overflow-hidden flex flex-col min-h-0">
            {
                move || {
                    if let Some(id) = selected.get() {
                        view! { <Dossier id={ id } /> }
                    } else {
                        view! {
                            <div class="px-4 py-16 text-center text-sm text-slate-500">
                                "Select an incident from the queue to inspect its dossier."
                            </div>
                        }
                    }
                }
            }
        </section>
    }
}

#[component]
pub fn Dossier(id: i64) -> DomNode {
    let app = use_context::<AppState>().expect("AppState must be provided before mounting");

    let incidents = app.incidents.clone();
    let incident = memo!(move || incidents.get().into_iter().find(|e| e.id == id));

    let note_text = signal!(String::new());
    let assignee_text = signal!(String::new());
    let pm_content = signal!(String::new());
    let pm_error = signal!(String::new());
    let dossier_error = app.api_error;

    // Refresh once when opened.
    {
        let app = app.clone();
        wasm_bindgen_futures::spawn_local(async move {
            crate::net::refresh_dossier(&app, id);
        });
    }

    view! {
        <div class="flex flex-col min-h-0">
            {
                move || {
                    if let Some(inc) = incident.get() {
                        // One owned local per view slot: each `move ||` slot
                        // captures exactly one value.
                        let sev = inc.sev();
                        let sev_label = sev.as_str().to_string();
                        let sev_badge = class_names!("text-[10px] font-bold px-1.5 py-0.5 rounded", sev.badge());
                        let status = inc.status.clone();
                        let status_badge = class_names!("text-[10px] font-bold px-1.5 py-0.5 rounded", status_pill(&status));
                        let key_line = format!("#{} · {}", inc.id, inc.correlation_key);
                        let title = inc.title.clone();
                        let stats_line = format!("{} · {} alerts · {} → {}", inc.entity, inc.alert_count, format_date_time(inc.first_seen_ms), format_date_time(inc.last_seen_ms));
                        let assignee_line = format!("assignee: {}", if inc.assignee.is_empty() { "-" } else { inc.assignee.as_str() });
                        let status_for_actions = status.clone();
                        view! {
                            <div>
                                <div class="px-4 py-3 border-b border-edge">
                                    <div class="flex items-center gap-2 flex-wrap">
                                        <span class={ sev_badge }>{ sev_label }</span>
                                        <span class={ status_badge }>{ status }</span>
                                        <span class="font-mono text-[11px] text-slate-500">{ key_line }</span>
                                    </div>
                                    <h2 class="text-base font-bold mt-1.5">{ title }</h2>
                                    <div class="font-mono text-[11px] text-slate-500 mt-0.5">
                                        { stats_line }
                                    </div>
                                    <div class="font-mono text-[11px] text-slate-500 mt-0.5">
                                        { assignee_line }
                                    </div>
                                </div>
                                <DossierActions id={ id } current_status={ status_for_actions } assignee_text={ assignee_text } />
                            </div>
                        }
                    } else {
                        view! { <div class="px-4 py-10 text-center text-xs text-slate-500">"Loading dossier…"</div> }
                    }
                }
            }
            <div class="px-4 py-1 text-xs font-mono text-rose-400">{ dossier_error }</div>
            <RunList />
            <AlertList />
            <NoteComposer id={ id } note_text={ note_text } />
            <TimelineView />
            <PostmortemView id={ id } pm_content={ pm_content } pm_error={ pm_error } />
        </div>
    }
}

/// Status dropdown, assignee field, and playbook launcher.
#[component]
pub fn DossierActions(id: i64, current_status: String, assignee_text: RwSignal<String>) -> DomNode {
    let app = use_context::<AppState>().expect("AppState must be provided before mounting");

    let on_status = {
        let app = app.clone();
        move |ev: Event| {
            let Some(target) = ev.target() else { return };
            let Ok(select) = target.dyn_into::<web_sys::HtmlSelectElement>() else { return };
            let value = select.value();
            let app = app.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match crate::api::set_status(id, &value).await {
                    Ok(inc) => {
                        app.api_error.set(String::new());
                        app.upsert_incident(inc);
                        crate::net::refresh_dossier(&app, id);
                    }
                    Err(e) => {
                        if e.is_forbidden() { app.banned.set(true); }
                        app.api_error.set(e.to_string());
                    }
                }
            });
        }
    };

    let on_assign = {
        let app = app.clone();
        move |_| {
            let who = assignee_text.get().trim().to_string();
            if who.is_empty() { return; }
            assignee_text.set(String::new());
            let app = app.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match crate::api::assign(id, &who).await {
                    Ok(inc) => {
                        app.api_error.set(String::new());
                        app.upsert_incident(inc);
                        crate::net::refresh_dossier(&app, id);
                    }
                    Err(e) => {
                        if e.is_forbidden() { app.banned.set(true); }
                        app.api_error.set(e.to_string());
                    }
                }
            });
        }
    };

    let on_run = {
        let app = app.clone();
        move |_| {
            let app = app.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match crate::api::start_run(id, None).await {
                    Ok(_) => {
                        app.api_error.set(String::new());
                        crate::net::refresh_dossier(&app, id);
                    }
                    Err(e) => {
                        if e.is_forbidden() { app.banned.set(true); }
                        app.api_error.set(e.to_string());
                    }
                }
            });
        }
    };

    view! {
        <div class="px-4 py-3 border-b border-edge flex flex-wrap items-center gap-2 bg-panel2/50">
            <select
                class="bg-ink border border-edge rounded-md px-2 py-1.5 text-xs text-slate-200"
                on:change={ on_status }
            >
                {
                    STATUSES.into_iter().map(|s| {
                        let label = s.to_string();
                        let val = label.clone();
                        if label == current_status {
                            view! { <option value={ val } selected>{ label }</option> }
                        } else {
                            view! { <option value={ val }>{ label }</option> }
                        }
                    }).collect::<Vec<_>>()
                }
            </select>
            <input
                type="text"
                class="bg-ink border border-edge rounded-md px-2.5 py-1.5 text-xs font-mono text-slate-200 w-32 focus:outline-none focus:border-accdim"
                placeholder="assignee…"
                bind:value={ assignee_text }
            />
            <button
                class="bg-panel2 border border-edge rounded-md px-3 py-1.5 text-xs font-semibold text-slate-300 hover:border-accdim hover:text-acc"
                on:click={ on_assign }>
                "Assign"
            </button>
            <button
                class="bg-acc text-ink rounded-md px-3 py-1.5 text-xs font-bold hover:brightness-110"
                on:click={ on_run }>
                "▶ Run playbook"
            </button>
        </div>
    }
}

/// Linked alerts (newest first from the server). The dossier signals always
/// hold the selected incident's data (replaced on every select), so no
/// per-row filtering is needed.
#[component]
pub fn AlertList() -> DomNode {
    let app = use_context::<AppState>().expect("AppState must be provided before mounting");
    let alerts = app.dossier_alerts.clone();
    let count_list = app.dossier_alerts.clone();
    let count = memo!(move || count_list.len().to_string());

    view! {
        <div class="px-4 py-2 border-b border-edge">
            <h3 class="text-[11px] font-bold tracking-[2px] text-slate-500 mb-1.5">{ "ALERTS (" } { count } { ")" }</h3>
            <div class="scrollthin max-h-44 overflow-y-auto flex flex-col gap-1.5">
                {
                    for a in alerts key = |a: &crate::state::Alert| a.id {
                        <AlertRow alert={ a.clone() } />
                    }
                }
            </div>
        </div>
    }
}

#[component]
pub fn AlertRow(alert: crate::state::Alert) -> DomNode {
    let sev = Severity::parse(&alert.severity);
    view! {
        <div class="border border-edge rounded px-2.5 py-1.5 bg-ink">
            <div class="flex items-center gap-2 text-[11px]">
                <span class={ class_names!("font-bold px-1.5 py-px rounded text-[10px]", sev.badge()) }>{ sev.as_str().to_string() }</span>
                <span class="font-mono text-slate-400">{ alert.kind.clone() }</span>
                <span class="font-mono text-slate-500">{ alert.entity.clone() }</span>
            </div>
            <div class="font-mono text-[11px] text-slate-500 mt-0.5 truncate">{ short_json(&alert.detail, 160) }</div>
        </div>
    }
}

/// Playbook runs with approval gates and expandable steps.
#[component]
pub fn RunList() -> DomNode {
    let app = use_context::<AppState>().expect("AppState must be provided before mounting");
    let runs = app.dossier_runs.clone();
    let count_list = app.dossier_runs.clone();
    let count = memo!(move || count_list.len().to_string());

    view! {
        <div class="px-4 py-2 border-b border-edge">
            <h3 class="text-[11px] font-bold tracking-[2px] text-slate-500 mb-1.5">{ "RUNS (" } { count } { ")" }</h3>
            <div class="flex flex-col gap-2">
                {
                    for r in runs key = |r: &Run| r.id {
                        <RunCard run={ r.clone() } />
                    }
                }
            </div>
        </div>
    }
}

#[component]
pub fn RunCard(run: Run) -> DomNode {
    let app = use_context::<AppState>().expect("AppState must be provided before mounting");
    let run_id = run.id;
    let expanded = signal!(false);

    // This card's steps live in a local list, loaded on expand.
    let steps = signal_vec(Vec::<RunStep>::new());

    let on_toggle = {
        let app = app.clone();
        let steps = steps.clone();
        move |_| {
            let open = !expanded.get();
            expanded.set(open);
            if open {
                wasm_bindgen_futures::spawn_local(async move {
                    match crate::api::get_run_detail(run_id).await {
                        Ok(d) => {
                            app.api_error.set(String::new());
                            let mut ordered = d.steps;
                            ordered.sort_by_key(|s| s.seq);
                            steps.with_mut(|v| *v = ordered);
                        }
                        Err(e) => {
                            if e.is_forbidden() { app.banned.set(true); }
                            app.api_error.set(e.to_string());
                        }
                    }
                });
            }
        }
    };

    let gate_open = run.status == "waiting_approval";
    let gate_label = if gate_open {
        "AWAITING APPROVAL".to_string()
    } else {
        run.status.clone()
    };

    view! {
        <div class="border border-edge rounded-md bg-ink overflow-hidden">
            <button class="w-full flex items-center gap-2 px-3 py-2 text-left hover:bg-panel2/60" on:click={ on_toggle }>
                <span class="text-slate-500 text-xs">{ "▸" }</span>
                <span class="font-mono text-xs text-slate-300">{ format!("run #{run_id}") }</span>
                <span class={ class_names!("text-[10px] font-bold px-1.5 py-px rounded", run_pill(&run.status)) }>{ gate_label }</span>
                <span class="font-mono text-[11px] text-slate-600">{ format_date_time(run.started_at as f64) }</span>
            </button>
            <div style:display={ move || if expanded.get() { "block".to_string() } else { "none".to_string() } }>
                {
                    for s in steps key = |s: &RunStep| s.id {
                        <StepRow step={ s.clone() } />
                    }
                }
            </div>
            {
                move || if gate_open {
                    view! { <GateButtons run_id={ run_id } steps={ steps } /> }
                } else {
                    DomNode::empty()
                }
            }
        </div>
    }
}

#[component]
pub fn StepRow(step: RunStep) -> DomNode {
    let seq_label = format!("#{}", step.seq);
    let name = step.name.clone();
    let status = step.status.clone();
    let status_badge = class_names!("text-[10px] font-bold px-1.5 py-px rounded", run_pill(&status));
    let gate_note = if step.gate {
        format!(" · gate:{}", step.gate_status)
    } else {
        String::new()
    };
    view! {
        <div class="px-3 py-1.5 border-t border-edge flex items-center gap-2 text-xs">
            <span class="font-mono text-slate-500">{ seq_label }</span>
            <span class="text-slate-200">{ name }</span>
            <span class={ status_badge }>{ status }</span>
            <span class="font-mono text-[11px] text-amber-300">{ gate_note }</span>
        </div>
    }
}

/// Approve / reject buttons for the open gate of a run.
#[component]
pub fn GateButtons(run_id: i64, steps: SignalVec<RunStep>) -> DomNode {
    let app = use_context::<AppState>().expect("AppState must be provided before mounting");

    // Resolve the open gate's seq from the loaded steps.
    let open_seq = memo!(move || {
        steps
            .get()
            .into_iter()
            .find(|s| s.run_id == run_id && s.gate_status == "open")
            .map(|s| s.seq)
            .unwrap_or(0)
    });

    // One state clone per button; each handler clones again per click so the
    // listeners stay FnMut.
    let app_approve = app.clone();
    let app_reject = app.clone();

    view! {
        <div class="px-3 py-2 border-t border-amber-500/30 bg-amber-500/5 flex items-center gap-2">
            <span class="text-xs text-amber-300">"Gate open — analyst decision required"</span>
            <button
                class="bg-emerald-500 text-ink rounded px-3 py-1 text-xs font-bold hover:brightness-110"
                on:click={ move |_| {
                    let app = app_approve.clone();
                    let seq = open_seq.get();
                    wasm_bindgen_futures::spawn_local(async move {
                        match crate::api::approve_step(run_id, seq, true).await {
                            Ok(()) => {
                                app.api_error.set(String::new());
                            }
                            Err(e) => {
                                if e.is_forbidden() { app.banned.set(true); }
                                app.api_error.set(e.to_string());
                            }
                        }
                    });
                } }>
                "Approve"
            </button>
            <button
                class="bg-transparent border border-rose-500 text-rose-400 rounded px-3 py-1 text-xs hover:bg-rose-500/10"
                on:click={ move |_| {
                    let app = app_reject.clone();
                    let seq = open_seq.get();
                    wasm_bindgen_futures::spawn_local(async move {
                        match crate::api::approve_step(run_id, seq, false).await {
                            Ok(()) => {
                                app.api_error.set(String::new());
                            }
                            Err(e) => {
                                if e.is_forbidden() { app.banned.set(true); }
                                app.api_error.set(e.to_string());
                            }
                        }
                    });
                } }>
                "Reject"
            </button>
        </div>
    }
}

/// Analyst note composer.
#[component]
pub fn NoteComposer(id: i64, note_text: RwSignal<String>) -> DomNode {
    let app = use_context::<AppState>().expect("AppState must be provided before mounting");

    view! {
        <div class="px-4 py-2 border-b border-edge flex gap-2">
            <input
                type="text"
                class="flex-1 bg-ink border border-edge rounded-md px-2.5 py-1.5 text-xs text-slate-200 focus:outline-none focus:border-accdim"
                placeholder="add a timeline note…"
                bind:value={ note_text }
            />
            <button
                class="bg-panel2 border border-edge rounded-md px-3 py-1.5 text-xs font-semibold text-slate-300 hover:border-accdim hover:text-acc"
                on:click={ move |_| {
                    let text = note_text.get().trim().to_string();
                    if text.is_empty() { return; }
                    note_text.set(String::new());
                    let app = app.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        match crate::api::add_note(id, &text).await {
                            Ok(_) => {
                                app.api_error.set(String::new());
                                crate::net::refresh_dossier(&app, id);
                            }
                            Err(e) => {
                                if e.is_forbidden() { app.banned.set(true); }
                                app.api_error.set(e.to_string());
                            }
                        }
                    });
                } }>
                "Note"
            </button>
        </div>
    }
}

/// Append-only audit timeline (ascending from the server).
#[component]
pub fn TimelineView() -> DomNode {
    let app = use_context::<AppState>().expect("AppState must be provided before mounting");
    let events = app.dossier_timeline.clone();
    let count_list = app.dossier_timeline.clone();
    let count = memo!(move || count_list.len().to_string());

    view! {
        <div class="px-4 py-2 border-b border-edge">
            <h3 class="text-[11px] font-bold tracking-[2px] text-slate-500 mb-1.5">{ "TIMELINE (" } { count } { ")" }</h3>
            <div class="scrollthin max-h-56 overflow-y-auto flex flex-col">
                {
                    for e in events key = |e: &crate::state::TimelineEvent| e.id {
                        <TimelineRow event={ e.clone() } />
                    }
                }
            </div>
        </div>
    }
}

#[component]
pub fn TimelineRow(event: crate::state::TimelineEvent) -> DomNode {
    let time = format_time(event.ts_ms);
    let kind = event.kind.clone();
    let actor = format!(" · {}", event.actor);
    let payload = short_json(&event.payload, 140);
    view! {
        <div class="flex gap-2.5 py-1 border-l border-edge pl-3 ml-1">
            <span class="font-mono text-[10px] text-slate-500 whitespace-nowrap">{ time }</span>
            <div class="min-w-0">
                <div class="text-xs">
                    <span class="font-semibold text-slate-200">{ kind }</span>
                    <span class="font-mono text-[10px] text-slate-500">{ actor }</span>
                </div>
                <div class="font-mono text-[11px] text-slate-500 truncate">{ payload }</div>
            </div>
        </div>
    }
}

/// Postmortem viewer + generator (Markdown rendered raw in a mono block).
#[component]
pub fn PostmortemView(id: i64, pm_content: RwSignal<String>, pm_error: RwSignal<String>) -> DomNode {
    let app = use_context::<AppState>().expect("AppState must be provided before mounting");

    {
        wasm_bindgen_futures::spawn_local(async move {
            match crate::api::get_postmortem(id).await {
                Ok(pm) => {
                    pm_content.set(pm.content);
                    pm_error.set(String::new());
                }
                Err(_) => {
                    pm_content.set(String::new());
                }
            }
        });
    }

    let has_doc = move || !pm_content.get().is_empty();

    view! {
        <div class="px-4 py-3">
            <div class="flex items-center justify-between mb-1.5">
                <h3 class="text-[11px] font-bold tracking-[2px] text-slate-500">"POSTMORTEM"</h3>
                <button
                    class="bg-panel2 border border-edge rounded-md px-3 py-1 text-xs font-semibold text-slate-300 hover:border-accdim hover:text-acc"
                    on:click={ move |_| {
                        let app = app.clone();
                        wasm_bindgen_futures::spawn_local(async move {
                            match crate::api::create_postmortem(id).await {
                                Ok(content) => {
                                    pm_content.set(content);
                                    pm_error.set(String::new());
                                    app.api_error.set(String::new());
                                }
                                Err(e) => {
                                    if e.is_forbidden() { app.banned.set(true); }
                                    pm_error.set(e.to_string());
                                }
                            }
                        });
                    } }>
                    { move || if has_doc() { "Regenerate" } else { "Generate" } }
                </button>
            </div>
            <div class="text-xs font-mono text-rose-400 mb-1">{ pm_error }</div>
            {
                move || if has_doc() {
                    view! { <pre class="scrollthin whitespace-pre-wrap break-words font-mono text-xs text-slate-300 bg-ink border border-edge rounded-md p-3 max-h-64 overflow-y-auto">{ pm_content.get() }</pre> }
                } else {
                    view! { <div class="text-xs text-slate-600">"No postmortem yet — generate one from the timeline."</div> }
                }
            }
        </div>
    }
}
