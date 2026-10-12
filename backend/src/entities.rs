use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Correlated security incidents (the top-level object operators work with).
pub mod incidents {
    use super::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
    #[sea_orm(table_name = "incidents")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub correlation_key: String,
        pub status: String,
        pub severity: String,
        pub title: String,
        pub summary: String,
        pub source: String,
        pub entity: String,
        pub alert_count: i64,
        pub first_seen_ms: f64,
        pub last_seen_ms: f64,
        pub assignee: String,
        pub created_at: i64,
        pub updated_at: i64,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// Raw alerts that feed incident correlation.
pub mod alerts {
    use super::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
    #[sea_orm(table_name = "alerts")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        /// `0` until the alert is attached to an incident.
        pub incident_id: i64,
        pub source: String,
        pub kind: String,
        pub severity: String,
        pub entity: String,
        pub detail: String,
        pub correlation_key: String,
        pub received_at: i64,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// Playbook definitions (runbook step graphs).
pub mod playbooks {
    use super::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
    #[sea_orm(table_name = "playbooks")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub name: String,
        pub description: String,
        pub auto_severity: String,
        /// Serialized `PlaybookDef::canonical()` steps JSON.
        pub steps: String,
        pub enabled: i64,
        pub created_at: i64,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// One playbook execution bound to an incident.
pub mod playbook_runs {
    use super::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
    #[sea_orm(table_name = "playbook_runs")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub playbook_id: i64,
        pub incident_id: i64,
        pub status: String,
        pub initiator: String,
        pub started_at: i64,
        pub finished_at: i64,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// Executed steps of a playbook run (one row per step attempt).
pub mod run_steps {
    use super::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
    #[sea_orm(table_name = "run_steps")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub run_id: i64,
        pub seq: i64,
        pub name: String,
        pub status: String,
        pub gate: i64,
        pub gate_status: String,
        /// JSON output/result of the step.
        pub output: String,
        pub started_at: i64,
        pub finished_at: i64,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// Append-only audit/timeline entries per incident.
pub mod timeline_events {
    use super::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
    #[sea_orm(table_name = "timeline_events")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub incident_id: i64,
        pub run_id: i64,
        pub actor: String,
        pub kind: String,
        /// JSON payload describing the event.
        pub payload: String,
        pub ts_ms: f64,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// Generated postmortem documents.
pub mod postmortems {
    use super::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
    #[sea_orm(table_name = "postmortems")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub incident_id: i64,
        pub content: String,
        pub created_at: i64,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}