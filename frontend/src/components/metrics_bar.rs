//! Severity tiles for the open queue plus the selected incident's pulse.

use veloasm::prelude::*;

use crate::state::{AppState, Severity};

fn count_open(incidents: &[crate::state::Incident], want: Severity) -> usize {
    incidents
        .iter()
        .filter(|e| {
            !crate::state::is_terminal_status(&e.status) && Severity::parse(&e.severity) == want
        })
        .count()
}

#[component]
pub fn MetricsBar() -> DomNode {
    let app = use_context::<AppState>().expect("AppState must be provided before mounting");

    let list = app.incidents.clone();
    let crit = memo!(move || count_open(&list.get(), Severity::Critical).to_string());
    let high = memo!(move || count_open(&list.get(), Severity::High).to_string());
    let med = memo!(move || count_open(&list.get(), Severity::Medium).to_string());
    let low = memo!(move || count_open(&list.get(), Severity::Low).to_string());

    let runs = app.dossier_runs.clone();
    let awaiting = memo!(move || {
        runs.get()
            .iter()
            .filter(|r| r.status == "waiting_approval")
            .count()
            .to_string()
    });

    let total = app.metrics.incident_total;

    view! {
        <section class="grid grid-cols-2 md:grid-cols-6 gap-px bg-edge border-b border-edge">
            <div class="bg-panel px-5 py-3">
                <div class="text-[10px] tracking-[2px] text-slate-500">"CRITICAL"</div>
                <div class="font-mono text-2xl font-bold text-rose-400">{ crit }</div>
            </div>
            <div class="bg-panel px-5 py-3">
                <div class="text-[10px] tracking-[2px] text-slate-500">"HIGH"</div>
                <div class="font-mono text-2xl font-bold text-amber-300">{ high }</div>
            </div>
            <div class="bg-panel px-5 py-3">
                <div class="text-[10px] tracking-[2px] text-slate-500">"MEDIUM"</div>
                <div class="font-mono text-2xl font-bold text-slate-300">{ med }</div>
            </div>
            <div class="bg-panel px-5 py-3">
                <div class="text-[10px] tracking-[2px] text-slate-500">"LOW"</div>
                <div class="font-mono text-2xl font-bold text-sky-300">{ low }</div>
            </div>
            <div class="bg-panel px-5 py-3">
                <div class="text-[10px] tracking-[2px] text-slate-500">"AWAITING APPROVAL"</div>
                <div class="font-mono text-2xl font-bold text-amber-300">{ awaiting }</div>
            </div>
            <div class="bg-panel px-5 py-3">
                <div class="text-[10px] tracking-[2px] text-slate-500">"TRACKED"</div>
                <div class="font-mono text-2xl font-bold text-acc">{ total }</div>
            </div>
        </section>
    }
}
