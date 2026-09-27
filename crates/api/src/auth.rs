//! API-key check and per-client token-bucket rate limiting.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::error::ApiError;
use crate::AppState;

pub struct RateLimiter {
    rps: f64,
    burst: f64,
    buckets: Mutex<HashMap<String, (f64, Instant)>>,
}

impl RateLimiter {
    pub fn new(rps: u32) -> Arc<Self> {
        Arc::new(Self {
            rps: rps.max(1) as f64,
            burst: (rps.max(1) * 2) as f64,
            buckets: Mutex::new(HashMap::new()),
        })
    }

    pub fn allow(&self, client: &str) -> bool {
        let now = Instant::now();
        let mut buckets = self.buckets.lock().expect("limiter lock");
        if buckets.len() > 100_000 {
            // Drop idle clients so the map can't grow without bound.
            buckets.retain(|_, (_, t)| now.duration_since(*t).as_secs() < 60);
        }
        let (tokens, last) = buckets.entry(client.to_owned()).or_insert((self.burst, now));
        *tokens = (*tokens + now.duration_since(*last).as_secs_f64() * self.rps).min(self.burst);
        *last = now;
        if *tokens >= 1.0 {
            *tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

fn presented_key(req: &Request) -> Option<String> {
    if let Some(v) = req.headers().get("x-api-key").and_then(|v| v.to_str().ok()) {
        return Some(v.to_owned());
    }
    // Browsers can't set headers on WebSocket upgrades, so allow ?api_key= too.
    req.uri().query().and_then(|q| {
        q.split('&')
            .find_map(|kv| kv.strip_prefix("api_key="))
            .map(str::to_owned)
    })
}

pub async fn guard(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Response {
    if req.uri().path() == "/v1/health" {
        return next.run(req).await;
    }
    let key = presented_key(&req);
    let client = if state.cfg.api_keys.is_empty() {
        addr.ip().to_string()
    } else {
        match key {
            Some(k) if state.cfg.api_keys.iter().any(|a| a == &k) => format!("key:{k}"),
            _ => return ApiError::Unauthorized.into_response(),
        }
    };
    if !state.limiter.allow(&client) {
        return ApiError::RateLimited.into_response();
    }
    next.run(req).await
}
