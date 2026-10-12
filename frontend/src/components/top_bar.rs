//! Top status bar: brand, connector health, and stream state.

use veloasm::prelude::*;

use crate::state::AppState;

#[component]
pub fn TopBar() -> DomNode {
    let app = use_context::<AppState>().expect("AppState must be provided before mounting");

    let conn = app.conn;
    let dot = memo!(move || conn.get().dot().to_string());
    let label = memo!(move || conn.get().label().to_string());
    let endpoint = crate::api::STREAM_PATH;
    let connectors = app.connectors;

    view! {
        <header class="flex items-center justify-between px-5 py-3 bg-panel border-b border-edge">
            <div class="flex items-baseline gap-2">
                <span class="text-acc text-xl">"⬢"</span>
                <span class="font-extrabold tracking-[2px] text-base">"GRITIRT"</span>
                <span class="text-slate-500 text-[11px] tracking-[3px]">"RESPONSE PLANE"</span>
            </div>
            <div class="flex items-center gap-4 text-xs text-slate-400">
                <span class="px-2.5 py-0.5 rounded-full bg-panel2 border border-edge text-acc font-bold tracking-wider">{ connectors }</span>
                <span class="flex items-center gap-1.5">
                    <span class={ dot } style="width:8px;height:8px;border-radius:9999px;display:inline-block;"></span>
                    <span>{ label }</span>
                </span>
                <code class="font-mono text-[11px] text-slate-500">{ endpoint }</code>
            </div>
        </header>
    }
}

/// Ban banner: 401/403 means bad key or a banned viewer (every endpoint,
// including the stream, stays blocked until unbanned).
#[component]
pub fn BanBanner() -> DomNode {
    let app = use_context::<AppState>().expect("AppState must be provided before mounting");
    let banned = app.banned;

    view! {
        <div
            class="bg-rose-500/10 border-b border-rose-500 text-rose-300 px-5 py-2 text-xs font-mono"
            style:display={ move || if banned.get() { "block".to_string() } else { "none".to_string() } }
        >
            "⛔ 403 — bad API key or this viewer is banned."
        </div>
    }
}
