//! Presentational + stateful components for the GritIRT console.
//!
//! Each is annotated with `#[component]`. Shared state is never threaded as
//! props — components pull the single `AppState` out of context with
//! `use_context`. Styling is Tailwind utilities (see `input.css`); every
//! class string is a full literal so the Tailwind scanner sees it.

pub mod incident_detail;
pub mod incident_queue;
pub mod metrics_bar;
pub mod playbook_panel;
pub mod toast_stack;
pub mod top_bar;

pub use incident_detail::*;
pub use incident_queue::*;
pub use metrics_bar::*;
pub use playbook_panel::*;
pub use toast_stack::*;
pub use top_bar::*;
