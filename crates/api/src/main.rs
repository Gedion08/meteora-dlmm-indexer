//! REST + WebSocket API over the DLMM index. Read-only against Postgres; live updates
//! arrive from the indexer through `LISTEN dlmm_commit`.

mod auth;
mod error;
mod live;
mod routes;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::http::{HeaderValue, Method};
use clap::Parser;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use tower_http::compression::CompressionLayer;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

#[derive(Parser, Debug, Clone)]
#[command(name = "dlmm-api", version, about = "REST + WebSocket API for the Meteora DLMM index")]
pub struct Config {
    #[arg(long, env = "DATABASE_URL", hide_env_values = true)]
    pub database_url: String,

    #[arg(long, env = "API_ADDR", default_value = "0.0.0.0:8080")]
    pub addr: SocketAddr,

    #[arg(long, env = "API_DB_MAX_CONNECTIONS", default_value_t = 20)]
    pub db_max_connections: u32,

    /// Per-query limit so one expensive request can't hurt the indexer.
    #[arg(long, env = "API_STATEMENT_TIMEOUT_MS", default_value_t = 5000)]
    pub statement_timeout_ms: u64,

    /// Comma-separated API keys. Empty = open access (development only).
    #[arg(long, env = "API_KEYS", value_delimiter = ',', hide_env_values = true)]
    pub api_keys: Vec<String>,

    /// Requests per second per API key (or per IP without keys); burst is 2x.
    #[arg(long, env = "API_RATE_LIMIT_RPS", default_value_t = 20)]
    pub rate_limit_rps: u32,

    /// Comma-separated allowed browser origins; empty = any.
    #[arg(long, env = "API_CORS_ORIGINS", value_delimiter = ',')]
    pub cors_origins: Vec<String>,

    #[arg(long, env = "API_MAX_WS_CLIENTS", default_value_t = 1000)]
    pub max_ws_clients: usize,

    #[arg(long, env = "LOG_JSON", default_value_t = false)]
    pub log_json: bool,
}

#[derive(Clone)]
pub struct AppState {
    pub pool: sqlx::PgPool,
    pub live: Arc<live::Hub>,
    pub cfg: Arc<Config>,
    pub limiter: Arc<auth::RateLimiter>,
}

fn init_tracing(json: bool) {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,tower_http=warn,sqlx=warn"));
    let f = fmt().with_env_filter(filter).with_writer(std::io::stderr);
    if json {
        f.json().init();
    } else {
        f.init();
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let _ = dotenvy::dotenv();
    let cfg = Config::parse();
    init_tracing(cfg.log_json);

    let opts: PgConnectOptions = cfg.database_url.parse().context("invalid DATABASE_URL")?;
    let opts = opts
        .application_name("dlmm-api")
        .options([("statement_timeout", cfg.statement_timeout_ms.to_string())]);
    let pool = PgPoolOptions::new()
        .max_connections(cfg.db_max_connections)
        .acquire_timeout(Duration::from_secs(5))
        .connect_with(opts)
        .await
        .context("connecting to Postgres")?;

    let shutdown = tokio_util::sync::CancellationToken::new();
    let hub = live::Hub::new(cfg.max_ws_clients);
    tokio::spawn(live::run_listener(pool.clone(), hub.clone(), shutdown.clone()));

    let state = AppState {
        pool,
        live: hub,
        limiter: auth::RateLimiter::new(cfg.rate_limit_rps),
        cfg: Arc::new(cfg.clone()),
    };

    let cors = if cfg.cors_origins.is_empty() {
        CorsLayer::new().allow_origin(Any)
    } else {
        let origins: Vec<HeaderValue> = cfg.cors_origins.iter().filter_map(|o| o.parse().ok()).collect();
        CorsLayer::new().allow_origin(AllowOrigin::list(origins))
    }
    .allow_methods([Method::GET])
    .allow_headers(Any);

    let app = routes::router(state.clone())
        .layer(axum::middleware::from_fn_with_state(state.clone(), auth::guard))
        .layer(CompressionLayer::new())
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::GATEWAY_TIMEOUT,
            Duration::from_secs(15),
        ))
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(cfg.addr).await?;
    tracing::info!(addr = %cfg.addr, auth = !cfg.api_keys.is_empty(), "dlmm-api listening");
    let sd = shutdown.clone();
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async move {
            let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("install SIGTERM handler");
            tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
            tracing::info!("shutting down");
            sd.cancel();
        })
        .await?;
    Ok(())
}
