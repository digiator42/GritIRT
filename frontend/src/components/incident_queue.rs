//! Incident queue: severity-sorted triage inbox with status/severity/text
//! filters. Clicking a card selects the incident and hydrates its dossier.

use veloasm::prelude::*;

use crate::state::{format_date_time, AppState, Incident, STATUSES};

#[component]
pub fn IncidentCard(incident: Incident) -> DomNode {
    let app = use_context::<AppState>().expect("AppState must be provided before mounting");
    let selected = app.selected_id;
    let id = incident.id;

    let sev = incident.sev();
    let card_class = class_names!(
        "border border-edge border-l-4 rounded-md px-3 py-2.5 bg-panel2 cursor-pointer select-none hover:bg-edge/40 text-left w-full",
        sev.edge()
    );
    let badge_class = class_names!(
        "text-[10px] font-bold px-1.5 py-0.5 rounded",
        sev.badge()
    );

    view! {
        <button
            class={ card_class }
            class:ring-2={ move || selected.get() == Some(id) }
            class:ring-acc={ move || selected.get() == Some(id) }
            on:click={ move |_| {
                selected.set(Some(id));
                crate::net::refresh_dossier(&app, id);
            } }
        >
            <div class="flex items-center justify-between gap-2">
                <span class={ badge_class }>{ sev.as_str().to_string() }</span>
                <span class="font-mono text-[10px] text-slate-500">{ format!("#{}", id) }</span>
            </div>
            <div class="font-semibold text-[13px] mt-1 truncate">{ incident.title.clone() }</div>
            <div class="font-mono text-[11px] text-slate-500 mt-0.5 truncate">{ incident.entity.clone() }</div>
            <div class="flex items-center justify-between mt-1.5 text-[11px]">
                <span class="text-slate-400">{ incident.status.clone() }</span>
                <span class="font-mono text-slate-500">{ format!("{} alerts", incident.alert_count) }</span>
            </div>
            <div class="font-mono text-[10px] text-slate-600 mt-0.5">{ format_date_time(incident.last_seen_ms) }</div>
        </button>
    }
}

#[component]
pub fn IncidentQueue() -> DomNode {
    let app = use_context::<AppState>().expect("AppState must be provided before mounting");

    let for_list = app.visible.clone();
    let count_list = app.visible.clone();
    let empty_list = app.visible.clone();
    let shown = memo!(move || format!("{} in view", count_list.len()));
    let is_empty = memo!(move || empty_list.len() == 0);

    let filter_status = app.filter_status;
    let filter_severity = app.filter_severity;
    let query = app.filter_query.clone();
    let selected = app.selected_id;

    let status_val = app.filter_status;
    let sev_val = app.filter_severity;

    view! {
        <section class="bg-panel border border-edge rounded-lg overflow-hidden flex flex-col min-h-0">
            <div class="flex items-center justify-between px-4 py-2.5 border-b border-edge">
                <h2 class="m-0 text-xs font-bold tracking-[2px] text-slate-500">"INCIDENT QUEUE"</h2>
                <span class="font-mono text-[11px] text-slate-500">{ shown }</span>
            </div>

            <div class="p-3 border-b border-edge flex flex-col gap-2">
                <input
                    type="text"
                    class="w-full bg-ink border border-edge rounded-md px-3 py-1.5 font-mono text-xs text-slate-200 focus:outline-none focus:border-accdim"
                    placeholder="filter by title, entity, key…"
                    bind:value={ query }
                />
                <div class="flex gap-2">
                    <select
                        class="flex-1 bg-ink border border-edge rounded-md px-2 py-1.5 text-xs text-slate-300"
                        bind:value={ status_val }
                    >
                        <option value="">"all statuses"</option>
                        {
                            STATUSES.into_iter().map(|s| {
                                let label = s.to_string();
                                let val = label.clone();
                                view! { <option value={ val }>{ label }</option> }
                            }).collect::<Vec<_>>()
                        }
                    </select>
                    <select
                        class="flex-1 bg-ink border border-edge rounded-md px-2 py-1.5 text-xs text-slate-300"
                        bind:value={ sev_val }
                    >
                        <option value="">"all severities"</option>
                        <option value="critical">"critical"</option>
                        <option value="high">"high"</option>
                        <option value="medium">"medium"</option>
                        <option value="low">"low"</option>
                    </select>
                </div>
                <div class="flex gap-1.5">
                    <button
                        class="text-[11px] font-bold px-2.5 py-1 rounded-full bg-panel2 border border-edge text-slate-400"
                        class:bg-acc={ move || filter_severity.get().is_empty() }
                        class:text-ink={ move || filter_severity.get().is_empty() }
                        on:click={ move |_| filter_severity.set(String::new()) }>
                        "ALL"
                    </button>
                    <button
                        class="text-[11px] font-bold px-2.5 py-1 rounded-full bg-panel2 border border-edge text-slate-400"
                        class:bg-rose-500={ move || filter_severity.get() == "critical" }
                        class:text-white={ move || filter_severity.get() == "critical" }
                        on:click={ move |_| filter_severity.set("critical".to_string()) }>
                        "CRIT"
                    </button>
                    <button
                        class="text-[11px] font-bold px-2.5 py-1 rounded-full bg-panel2 border border-edge text-slate-400"
                        class:bg-amber-400={ move || filter_severity.get() == "high" }
                        class:text-ink={ move || filter_severity.get() == "high" }
                        on:click={ move |_| filter_severity.set("high".to_string()) }>
                        "HIGH"
                    </button>
                    <button
                        class="text-[11px] font-bold px-2.5 py-1 rounded-full bg-panel2 border border-edge text-slate-400"
                        class:bg-acc={ move || selected.get().is_some() }
                        class:text-ink={ move || selected.get().is_some() }
                        on:click={ move |_| {
                            selected.set(None);
                            filter_status.set(String::new());
                            filter_severity.set(String::new());
                        } }>
                        "RESET"
                    </button>
                </div>
            </div>

            <div class="scrollthin overflow-y-auto p-2.5 flex flex-col gap-2 max-h-[560px]">
                {
                    move || if is_empty.get() {
                        view! { <div class="px-3 py-10 text-center text-xs text-slate-500">"No incidents match. The stream connects on load."</div> }
                    } else {
                        DomNode::empty()
                    }
                }
                {
                    for e in for_list key = |e: &Incident| e.id {
                        <IncidentCard incident={ e.clone() } />
                    }
                }
            </div>
        </section>
    }
}
