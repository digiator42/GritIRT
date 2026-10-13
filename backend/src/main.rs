mod api;
mod db;
mod entities;
mod feed;
mod ingest;
#[cfg(feature = "mcp")]
mod mcp;
mod middleware;
mod playbook;
mod state;
mod types;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use gritshield::core::logger::LogLevel;
use gritshield::database::db::{DbConfig, DbManager};
use gritshield::prelude::*;

use crate::feed::Feed;
use crate::middleware::{ApiKeyMiddleware, BodyLimitMiddleware, HttpSecurityMiddleware};
use crate::state::{AppState, Connector, DEFAULT_DEDUP_WINDOW_MS};

#[tokio::main]
async fn main() {
    // --- Persistence: file-backed SQLite via the framework's DbManager ---
    let db_url = std::env::var("IRT_DATABASE_URL")
        .unwrap_or_else(|_| "sqlite://grit_irt.db?mode=rwc".to_string());
    let db_config = DbConfig {
        url: db_url.clone(),
        max_connections: 5,
        min_connections: 1,
        connect_timeout: Duration::from_secs(10),
        idle_timeout: Duration::from_secs(300),
    };

    let shared_db = DbManager::connect(db_config)
        .await
        .unwrap_or_else(|e| panic!("failed to connect to database at {db_url}: {e}"));

    db::init_tables(shared_db.as_ref())
        .await
        .expect("failed to initialize GritIRT schema");

    // --- Outbound connectors used by playbook MCP/HTTP steps ---
    let mut connectors = HashMap::new();
    if let Ok(base) = std::env::var("IRT_LUXSHIELD_URL") {
        connectors.insert(
            "luxshield".to_string(),
            Connector {
                base_url: base,
                api_key: std::env::var("IRT_LUXSHIELD_API_KEY").ok(),
            },
        );
    }
    if let Ok(base) = std::env::var("IRT_CYBERFLOW_URL") {
        connectors.insert(
            "cyberflow".to_string(),
            Connector {
                base_url: base,
                api_key: std::env::var("IRT_CYBERFLOW_API_KEY").ok(),
            },
        );
    }
    if let Ok(base) = std::env::var("IRT_ACC_URL") {
        connectors.insert(
            "acc".to_string(),
            Connector {
                base_url: base,
                api_key: std::env::var("IRT_ACC_API_KEY").ok(),
            },
        );
    }
    if let Ok(base) = std::env::var("IRT_GRITSCAN_URL") {
        connectors.insert(
            "gritscan".to_string(),
            Connector {
                base_url: base,
                api_key: std::env::var("IRT_GRITSCAN_API_KEY").ok(),
            },
        );
    }

    let api_key = std::env::var("IRT_API_KEY").ok().filter(|k| !k.is_empty());
    let dedup_window_ms = std::env::var("IRT_DEDUP_WINDOW_MS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_DEDUP_WINDOW_MS);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .expect("failed to build outbound HTTP client");

    let app_state = Arc::new(AppState {
        db: shared_db.clone(),
        feed: Arc::new(Feed::new()),
        api_key,
        connectors: Arc::new(connectors),
        dedup_window_ms,
        client,
        active_runs: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
    });
    gritshield::core::ioc::AutoWire::component((*app_state).clone());

    // --- Router with the GritIRT middleware chain (request phase runs in order) ---
    let router = Router::new()
        .mount_logger(LogLevel::Debug)
        .mount_db(shared_db)
        .add_middleware(gritshield::middleware::CorsMiddleware::new(vec![
            "*".to_string(),
        ]))
        .add_middleware(ApiKeyMiddleware::new(app_state.clone()))
        .add_middleware(BodyLimitMiddleware::new(1024 * 1024)) // 1 MiB
        .add_middleware(HttpSecurityMiddleware::new())
        .route(("/health", HttpMethod::GET, api::health))
        .route(("/api/v1/alerts", HttpMethod::POST, api::post_alert))
        .route(("/api/v1/incidents", HttpMethod::GET, api::list_incidents))
        .route(("/api/v1/incidents/:id", HttpMethod::GET, api::get_incident))
        .route(("/api/v1/incidents/:id/status", HttpMethod::POST, api::set_status))
        .route(("/api/v1/incidents/:id/assign", HttpMethod::POST, api::assign))
        .route(("/api/v1/incidents/:id/notes", HttpMethod::POST, api::add_note))
        .route(("/api/v1/incidents/:id/timeline", HttpMethod::GET, api::get_timeline))
        .route(("/api/v1/incidents/:id/alerts", HttpMethod::GET, api::get_alerts))
        .route(("/api/v1/incidents/:id/playbook", HttpMethod::POST, api::run_playbook))
        .route(("/api/v1/incidents/:id/runs", HttpMethod::GET, api::get_runs))
        .route(("/api/v1/incidents/:id/postmortem", HttpMethod::GET, api::get_postmortem))
        .route(("/api/v1/incidents/:id/postmortem", HttpMethod::POST, api::create_postmortem))
        .route(("/api/v1/playbooks", HttpMethod::GET, api::list_playbooks))
        .route(("/api/v1/playbooks", HttpMethod::POST, api::create_playbook))
        .route(("/api/v1/playbooks/:id", HttpMethod::GET, api::get_playbook))
        .route(("/api/v1/runs/:id", HttpMethod::GET, api::get_run))
        .route(("/api/v1/runs/:id/approve", HttpMethod::POST, api::approve_step))
        .route(("/api/v1/stream", HttpMethod::GET, api::stream));

    let host = std::env::var("IRT_BIND").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("IRT_PORT").unwrap_or_else(|_| "8030".to_string());

    println!("[gritirt] incident-response backend listening on http://{host}:{port}");
    println!(
        "[gritirt] MCP surface mounted automatically at http://{host}:{port}/mcp (MCP_HTTP_PREFIX to relocate)"
    );
    println!(
        "[gritirt] database: {}",
        if db_url.contains("memory") { db_url } else { format!("{db_url} (file)") }
    );

    ignite(&host, &port, router).await;
}