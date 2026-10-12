//! Playbook panel: runbook list with auto-severity badges, a create form
//! (steps as JSON), and one-click runs against the selected incident.

use veloasm::prelude::*;

use crate::state::{short_json, AppState, Playbook};

const DEFAULT_STEPS: &str = r#"[
  {"name": "note", "action": {"kind": "sim", "text": "triaging"}},
  {"name": "block", "gate": true, "action": {"kind": "mcp", "connector": "luxshield", "tool": "add_firewall_rule", "arguments": {"ip": "{{incident.entity}}"}}}
]"#;

fn start_playbook_poll(app: AppState) {
    let refresh = move || {
        let app = app.clone();
        wasm_bindgen_futures::spawn_local(async move {
            match crate::api::list_playbooks().await {
                Ok(playbooks) => {
                    app.banned.set(false);
                    app.api_error.set(String::new());
                    app.playbooks.with_mut(|v| *v = playbooks);
                }
                Err(e) => {
                    if e.is_forbidden() {
                        app.banned.set(true);
                    }
                    app.api_error.set(e.to_string());
                }
            }
        });
    };

    refresh();
    let tick = refresh.clone();
    let _interval = gloo_timers::callback::Interval::new(15_000, move || tick());
    std::mem::forget(_interval);
}

#[component]
pub fn PlaybookPanel() -> DomNode {
    let app = use_context::<AppState>().expect("AppState must be provided before mounting");

    let list = app.playbooks.clone();
    let count_list = app.playbooks.clone();
    let empty_list = app.playbooks.clone();
    let count = memo!(move || format!("{} defined", count_list.len()));
    let is_empty = memo!(move || empty_list.len() == 0);

    let name_input = signal!(String::new());
    let sev_input = signal!(String::new());
    let steps_input = signal!(DEFAULT_STEPS.to_string());
    let form_error = signal!(String::new());

    start_playbook_poll(app.clone());

    view! {
        <section class="bg-panel border border-edge rounded-lg overflow-hidden flex flex-col min-h-0">
            <div class="flex items-center justify-between px-4 py-2.5 border-b border-edge">
                <h2 class="m-0 text-xs font-bold tracking-[2px] text-slate-500">"PLAYBOOKS"</h2>
                <span class="font-mono text-[11px] text-slate-500">{ count }</span>
            </div>

            <div class="scrollthin max-h-64 overflow-y-auto p-2.5 flex flex-col gap-2">
                {
                    move || if is_empty.get() {
                        view! { <div class="px-2 py-6 text-center text-xs text-slate-500">"No playbooks yet. Create the first runbook below."</div> }
                    } else {
                        DomNode::empty()
                    }
                }
                {
                    for p in list key = |p: &Playbook| p.id {
                        <PlaybookCard playbook={ p.clone() } />
                    }
                }
            </div>

            <div class="p-3 border-t border-edge flex flex-col gap-2">
                <input
                    type="text"
                    class="bg-ink border border-edge rounded-md px-2.5 py-1.5 text-xs text-slate-200 focus:outline-none focus:border-accdim"
                    placeholder="runbook name…"
                    bind:value={ name_input }
                />
                <div class="flex gap-2">
                    <select
                        class="flex-1 bg-ink border border-edge rounded-md px-2 py-1.5 text-xs text-slate-300"
                        bind:value={ sev_input }
                    >
                        <option value="">"manual only"</option>
                        <option value="low">"auto ≥ low"</option>
                        <option value="medium">"auto ≥ medium"</option>
                        <option value="high">"auto ≥ high"</option>
                        <option value="critical">"auto ≥ critical"</option>
                    </select>
                    <button
                        class="bg-acc text-ink rounded-md px-3 py-1.5 text-xs font-bold hover:brightness-110 whitespace-nowrap"
                        on:click={ move |_| {
                            let name = name_input.get().trim().to_string();
                            let sev = sev_input.get().trim().to_string();
                            let steps_raw = steps_input.get();
                            let steps: serde_json::Value = match serde_json::from_str(&steps_raw) {
                                Ok(v) => v,
                                Err(e) => {
                                    form_error.set(format!("steps JSON: {e}"));
                                    return;
                                }
                            };
                            if name.is_empty() {
                                form_error.set("name_required".to_string());
                                return;
                            }
                            form_error.set(String::new());
                            let app = app.clone();
                            wasm_bindgen_futures::spawn_local(async move {
                                match crate::api::create_playbook(&name, "", &sev, &steps).await {
                                    Ok(_) => {
                                        name_input.set(String::new());
                                        app.api_error.set(String::new());
                                        match crate::api::list_playbooks().await {
                                            Ok(playbooks) => app.playbooks.with_mut(|v| *v = playbooks),
                                            Err(e) => app.api_error.set(e.to_string()),
                                        }
                                    }
                                    Err(e) => {
                                        if e.is_forbidden() { app.banned.set(true); }
                                        form_error.set(e.to_string());
                                    }
                                }
                            });
                        } }>
                        "Create"
                    </button>
                </div>
                <textarea
                    class="scrollthin bg-ink border border-edge rounded-md px-2.5 py-1.5 font-mono text-[11px] text-slate-300 h-24 focus:outline-none focus:border-accdim"
                    bind:value={ steps_input }
                ></textarea>
                <div class="text-[11px] font-mono text-rose-400">{ form_error }</div>
            </div>
        </section>
    }
}

#[component]
pub fn PlaybookCard(playbook: Playbook) -> DomNode {
    let app = use_context::<AppState>().expect("AppState must be provided before mounting");
    let selected = app.selected_id;
    let pb_id = playbook.id;

    let auto_badge = if playbook.auto_severity.is_empty() {
        "manual".to_string()
    } else {
        format!("auto ≥ {}", playbook.auto_severity)
    };

    view! {
        <div class="border border-edge rounded-md px-3 py-2 bg-ink">
            <div class="flex items-center justify-between gap-2">
                <span class="font-semibold text-[13px] truncate">{ playbook.name.clone() }</span>
                <span class="font-mono text-[10px] text-acc shrink-0">{ auto_badge }</span>
            </div>
            <div class="font-mono text-[11px] text-slate-500 mt-0.5 truncate">{ short_json(&playbook.steps, 120) }</div>
            <button
                class="mt-1.5 bg-panel2 border border-edge rounded px-2.5 py-1 text-[11px] font-semibold text-slate-300 hover:border-accdim hover:text-acc"
                class:opacity-40={ move || selected.get().is_none() }
                on:click={ move |_| {
                    let Some(id) = selected.get() else { return; };
                    let app = app.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        match crate::api::start_run(id, Some(pb_id)).await {
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
                "Run on selected"
            </button>
        </div>
    }
}
