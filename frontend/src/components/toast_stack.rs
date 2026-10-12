//! Flash alerts for new/updated incidents (auto-dismissed).

use veloasm::prelude::*;

use crate::state::{AppState, Toast};

#[component]
pub fn ToastCard(toast: Toast) -> DomNode {
    let app = use_context::<AppState>().expect("AppState must be provided before mounting");
    let id = toast.id;
    let toasts = app.toasts.clone();

    view! {
        <div class={ if toast.critical { "toast-in flex items-start justify-between gap-2 px-3 py-2.5 bg-panel2 border border-edge border-l-4 border-l-rose-500 rounded-lg shadow-2xl" } else { "toast-in flex items-start justify-between gap-2 px-3 py-2.5 bg-panel2 border border-edge border-l-4 border-l-acc rounded-lg shadow-2xl" } }>
            <div class="min-w-0">
                <div class="font-bold text-[13px] truncate">{ toast.title.clone() }</div>
                <div class="font-mono text-[11px] text-slate-500 truncate">{ toast.detail.clone() }</div>
            </div>
            <button
                class="bg-transparent border-none text-slate-500 text-base cursor-pointer px-0.5 hover:text-rose-400"
                on:click={ move |_| {
                    toasts.with_mut(|v| {
                        if let Some(pos) = v.iter().position(|t| t.id == id) {
                            v.remove(pos);
                        }
                    });
                }}>
                "×"
            </button>
        </div>
    }
}

#[component]
pub fn ToastStack() -> DomNode {
    let app = use_context::<AppState>().expect("AppState must be provided before mounting");
    let list = app.toasts.clone();

    view! {
        <div class="fixed top-4 right-4 flex flex-col gap-2 z-50 w-80">
            {
                for t in list key = |t: &Toast| t.id {
                    <ToastCard toast={ t.clone() } />
                }
            }
        </div>
    }
}
