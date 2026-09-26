//! Minimal JSON-RPC client with retries, used for snapshots and account refreshes.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

#[derive(Clone)]
pub struct Rpc {
    http: reqwest::Client,
    url: String,
}

impl Rpc {
    pub fn new(url: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .gzip(true)
            .build()?;
        Ok(Self { http, url })
    }

    pub async fn call<T: DeserializeOwned>(&self, method: &str, params: Value) -> Result<T> {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let mut delay = Duration::from_millis(500);
        for attempt in 1..=8 {
            match self.try_call(&body).await {
                Ok(v) => return serde_json::from_value(v).with_context(|| format!("decoding {method} result")),
                Err(e) if attempt < 8 => {
                    tracing::warn!(method, attempt, error = %e, "rpc call failed, retrying");
                    metrics::counter!("rpc_errors_total", "method" => method.to_owned()).increment(1);
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_secs(20));
                }
                Err(e) => return Err(e.context(format!("rpc {method} failed"))),
            }
        }
        unreachable!()
    }

    async fn try_call(&self, body: &Value) -> Result<Value> {
        let resp = self.http.post(&self.url).json(body).send().await?;
        let status = resp.status();
        if !status.is_success() {
            // Never log the URL: it carries the API key.
            bail!("http status {status}");
        }
        let mut v: Value = resp.json().await?;
        if let Some(err) = v.get("error") {
            return Err(anyhow!("rpc error: {err}"));
        }
        Ok(v.get_mut("result").map(Value::take).unwrap_or(Value::Null))
    }
}
