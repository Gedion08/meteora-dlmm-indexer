//! Live feed: one Postgres `LISTEN dlmm_commit` connection turns indexer commits into
//! messages, which are broadcast to every WebSocket client and filtered per subscription.
//!
//! Client protocol (JSON text frames):
//!   → {"op":"subscribe","channel":"swaps"|"events"|"pairs","lb_pair":?,"wallet":?,"names":?[..]}
//!   ← {"type":"subscribed","id":1,...}
//!   → {"op":"unsubscribe","id":1}      → {"op":"ping"}
//!   ← {"type":"swap","sub":1,"data":{...}}   (same shape as REST /swaps items)
//!   ← {"type":"event","sub":1,"data":{...}}  (same shape as REST /events items)
//!   ← {"type":"pair","sub":1,"data":{"address","slot","active_id","price","price_raw",...}}
//!   ← {"type":"lagged","missed":n} / {"type":"resync"}  — refetch via REST, then continue
//!
//! Delivery is at-most-once per connection; clients recover with REST using cursors.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::postgres::PgListener;
use sqlx::PgPool;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Swaps,
    Events,
    Pairs,
}

#[derive(Debug)]
pub enum LiveMsg {
    Item {
        channel: Channel,
        name: String,
        lb_pair: Option<String>,
        wallet: Option<String>,
        /// Pre-serialized `data` object, shared by all recipients.
        data: Arc<str>,
    },
    /// The listener reconnected; notifications may have been missed.
    Resync,
}

pub struct Hub {
    tx: broadcast::Sender<Arc<LiveMsg>>,
    clients: AtomicUsize,
    max_clients: usize,
}

impl Hub {
    pub fn new(max_clients: usize) -> Arc<Self> {
        let (tx, _) = broadcast::channel(8192);
        Arc::new(Self {
            tx,
            clients: AtomicUsize::new(0),
            max_clients,
        })
    }

    pub fn clients(&self) -> usize {
        self.clients.load(Ordering::Relaxed)
    }
}

// ---------- listener ----------

/// Remembers recently published keys so late re-deliveries of a slot aren't pushed twice.
struct Seen {
    set: HashSet<String>,
    order: VecDeque<String>,
}

impl Seen {
    fn insert(&mut self, k: String) -> bool {
        if !self.set.insert(k.clone()) {
            return false;
        }
        self.order.push_back(k);
        while self.order.len() > 200_000 {
            if let Some(old) = self.order.pop_front() {
                self.set.remove(&old);
            }
        }
        true
    }
}

pub async fn run_listener(pool: PgPool, hub: Arc<Hub>, shutdown: CancellationToken) {
    let mut seen = Seen { set: HashSet::new(), order: VecDeque::new() };
    loop {
        let mut listener = match PgListener::connect_with(&pool).await {
            Ok(l) => l,
            Err(e) => {
                tracing::error!(error = %e, "live listener connect failed; retrying");
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };
        if let Err(e) = listener.listen("dlmm_commit").await {
            tracing::error!(error = %e, "LISTEN failed; retrying");
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }
        tracing::info!("live listener subscribed to dlmm_commit");
        let _ = hub.tx.send(Arc::new(LiveMsg::Resync));
        loop {
            let n = tokio::select! {
                _ = shutdown.cancelled() => return,
                n = listener.try_recv() => n,
            };
            match n {
                Ok(Some(n)) => {
                    let slots: Vec<i64> = serde_json::from_str::<Value>(n.payload())
                        .ok()
                        .and_then(|v| serde_json::from_value(v["slots"].clone()).ok())
                        .unwrap_or_default();
                    if slots.is_empty() {
                        continue;
                    }
                    if let Err(e) = publish(&pool, &hub, &mut seen, &slots).await {
                        tracing::error!(error = %e, "publishing live update failed");
                    }
                }
                // Connection dropped and was re-established: anything in between is lost.
                Ok(None) => {
                    tracing::warn!("live listener reconnected; telling clients to resync");
                    let _ = hub.tx.send(Arc::new(LiveMsg::Resync));
                }
                Err(e) => {
                    tracing::error!(error = %e, "live listener error; reconnecting");
                    break;
                }
            }
        }
    }
}

async fn publish(pool: &PgPool, hub: &Hub, seen: &mut Seen, slots: &[i64]) -> Result<(), sqlx::Error> {
    // Same shapes as the REST endpoints so clients can share parsing code.
    let rows: Vec<(String, String, Option<String>, Option<String>, String, String)> = sqlx::query_as(
        "SELECT concat_ws('.', e.slot, e.signature, e.ix_index, e.inner_index) AS key,
                e.name, e.lb_pair, e.wallet,
                json_build_object(
                    'signature', e.signature, 'slot', e.slot, 'tx_index', e.tx_index, 'ix_index', e.ix_index,
                    'inner_index', e.inner_index, 'block_time', extract(epoch FROM e.block_time)::bigint,
                    'name', e.name, 'lb_pair', e.lb_pair, 'position', e.position, 'wallet', e.wallet,
                    'data', e.data,
                    'cursor', concat_ws('.', e.slot, e.tx_index, e.ix_index, e.inner_index))::text AS event_json,
                CASE WHEN e.name = 'Swap' THEN json_build_object(
                    'signature', e.signature, 'slot', e.slot, 'tx_index', e.tx_index, 'ix_index', e.ix_index,
                    'inner_index', e.inner_index, 'block_time', extract(epoch FROM e.block_time)::bigint,
                    'lb_pair', e.lb_pair, 'trader', e.wallet,
                    'swap_for_y', (e.data->>'swap_for_y')::bool,
                    'amount_in', e.data->>'amount_in', 'amount_out', e.data->>'amount_out',
                    'fee', e.data->>'fee', 'protocol_fee', e.data->>'protocol_fee', 'host_fee', e.data->>'host_fee',
                    'fee_pct', (e.data->>'fee_bps')::numeric / 1e7,
                    'start_bin_id', (e.data->>'start_bin_id')::int, 'end_bin_id', (e.data->>'end_bin_id')::int,
                    'price_raw', dlmm_bin_price((e.data->>'end_bin_id')::int, (p.data->>'bin_step')::int),
                    'price', CASE WHEN mx.decimals IS NOT NULL AND my.decimals IS NOT NULL THEN
                        dlmm_ui_price((e.data->>'end_bin_id')::int, (p.data->>'bin_step')::int, mx.decimals, my.decimals) END,
                    'cursor', concat_ws('.', e.slot, e.tx_index, e.ix_index, e.inner_index))::text
                ELSE '' END AS swap_json
         FROM events e
         LEFT JOIN accounts p ON p.pubkey = e.lb_pair
         LEFT JOIN mints mx ON mx.mint = p.data->>'token_x_mint'
         LEFT JOIN mints my ON my.mint = p.data->>'token_y_mint'
         WHERE e.slot = ANY($1)
         ORDER BY e.slot, e.tx_index, e.ix_index, e.inner_index",
    )
    .bind(slots)
    .fetch_all(pool)
    .await?;

    for (key, name, lb_pair, wallet, event_json, swap_json) in rows {
        if !seen.insert(key) {
            continue;
        }
        if name == "Swap" {
            let _ = hub.tx.send(Arc::new(LiveMsg::Item {
                channel: Channel::Swaps,
                name: name.clone(),
                lb_pair: lb_pair.clone(),
                wallet: wallet.clone(),
                data: swap_json.into(),
            }));
        }
        let _ = hub.tx.send(Arc::new(LiveMsg::Item {
            channel: Channel::Events,
            name,
            lb_pair,
            wallet,
            data: event_json.into(),
        }));
    }

    let pairs: Vec<(String, i64, String)> = sqlx::query_as(
        "SELECT a.pubkey, a.slot, json_build_object(
                'address', a.pubkey, 'slot', a.slot,
                'active_id', (a.data->>'active_id')::int, 'bin_step', (a.data->>'bin_step')::int,
                'price_raw', dlmm_bin_price((a.data->>'active_id')::int, (a.data->>'bin_step')::int),
                'price', CASE WHEN mx.decimals IS NOT NULL AND my.decimals IS NOT NULL THEN
                    dlmm_ui_price((a.data->>'active_id')::int, (a.data->>'bin_step')::int, mx.decimals, my.decimals) END,
                'volatility_accumulator', (a.data->'v_parameters'->>'volatility_accumulator')::bigint,
                'protocol_fee_x', a.data->'protocol_fee'->>'amount_x',
                'protocol_fee_y', a.data->'protocol_fee'->>'amount_y')::text
         FROM accounts a
         LEFT JOIN mints mx ON mx.mint = a.data->>'token_x_mint'
         LEFT JOIN mints my ON my.mint = a.data->>'token_y_mint'
         WHERE a.account_type = 'LbPair' AND a.slot = ANY($1)",
    )
    .bind(slots)
    .fetch_all(pool)
    .await?;
    for (pubkey, slot, data) in pairs {
        if !seen.insert(format!("pair.{pubkey}.{slot}")) {
            continue;
        }
        let _ = hub.tx.send(Arc::new(LiveMsg::Item {
            channel: Channel::Pairs,
            name: "LbPair".into(),
            lb_pair: Some(pubkey),
            wallet: None,
            data: data.into(),
        }));
    }
    Ok(())
}

// ---------- websocket clients ----------

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum ClientMsg {
    Subscribe {
        channel: Channel,
        lb_pair: Option<String>,
        wallet: Option<String>,
        names: Option<Vec<String>>,
    },
    Unsubscribe {
        id: u32,
    },
    Ping,
}

struct Sub {
    channel: Channel,
    lb_pair: Option<String>,
    wallet: Option<String>,
    names: Option<HashSet<String>>,
}

impl Sub {
    fn matches(&self, msg: &LiveMsg) -> bool {
        let LiveMsg::Item { channel, name, lb_pair, wallet, .. } = msg else {
            return false;
        };
        *channel == self.channel
            && self.lb_pair.as_ref().is_none_or(|p| lb_pair.as_ref() == Some(p))
            && self.wallet.as_ref().is_none_or(|w| wallet.as_ref() == Some(w))
            && self.names.as_ref().is_none_or(|n| n.contains(name))
    }
}

const MAX_SUBS_PER_CLIENT: usize = 50;

pub async fn ws_handler(State(s): State<AppState>, ws: WebSocketUpgrade) -> Response {
    if s.live.clients() >= s.live.max_clients {
        return (axum::http::StatusCode::SERVICE_UNAVAILABLE, "too many live clients").into_response();
    }
    ws.max_message_size(64 * 1024)
        .on_upgrade(move |socket| client(socket, s.live.clone()))
}

async fn client(socket: WebSocket, hub: Arc<Hub>) {
    hub.clients.fetch_add(1, Ordering::Relaxed);
    let mut rx = hub.tx.subscribe();
    let (mut out, mut inbox) = socket.split();
    let mut subs: HashMap<u32, Sub> = HashMap::new();
    let mut next_id = 1u32;
    let mut ping = tokio::time::interval(Duration::from_secs(20));
    let mut last_seen = tokio::time::Instant::now();

    let result: Result<(), axum::Error> = async {
        loop {
            tokio::select! {
                msg = inbox.next() => {
                    let Some(Ok(msg)) = msg else { return Ok(()) };
                    last_seen = tokio::time::Instant::now();
                    let Message::Text(text) = msg else { continue };
                    let reply = match serde_json::from_str::<ClientMsg>(&text) {
                        Ok(ClientMsg::Subscribe { channel, lb_pair, wallet, names }) => {
                            if subs.len() >= MAX_SUBS_PER_CLIENT {
                                json!({"type": "error", "message": "too many subscriptions"})
                            } else {
                                let id = next_id;
                                next_id += 1;
                                let r = json!({"type": "subscribed", "id": id, "channel": channel_name(channel),
                                               "lb_pair": lb_pair, "wallet": wallet, "names": names});
                                subs.insert(id, Sub { channel, lb_pair, wallet, names: names.map(|n| n.into_iter().collect()) });
                                r
                            }
                        }
                        Ok(ClientMsg::Unsubscribe { id }) => {
                            json!({"type": "unsubscribed", "id": id, "found": subs.remove(&id).is_some()})
                        }
                        Ok(ClientMsg::Ping) => json!({"type": "pong"}),
                        Err(e) => json!({"type": "error", "message": format!("bad message: {e}")}),
                    };
                    out.send(Message::Text(reply.to_string().into())).await?;
                }
                m = rx.recv() => match m {
                    Ok(m) => {
                        if matches!(*m, LiveMsg::Resync) {
                            out.send(Message::Text(r#"{"type":"resync"}"#.into())).await?;
                            continue;
                        }
                        for (id, sub) in &subs {
                            if sub.matches(&m) {
                                if let LiveMsg::Item { channel, data, .. } = &*m {
                                    let kind = match channel { Channel::Swaps => "swap", Channel::Events => "event", Channel::Pairs => "pair" };
                                    let frame = format!(r#"{{"type":"{kind}","sub":{id},"data":{data}}}"#);
                                    out.send(Message::Text(frame.into())).await?;
                                }
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        out.send(Message::Text(format!(r#"{{"type":"lagged","missed":{n}}}"#).into())).await?;
                    }
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                },
                _ = ping.tick() => {
                    if last_seen.elapsed() > Duration::from_secs(60) {
                        return Ok(()); // unresponsive client
                    }
                    out.send(Message::Ping(Vec::new().into())).await?;
                }
            }
        }
    }
    .await;
    if let Err(e) = result {
        tracing::debug!(error = %e, "websocket client closed");
    }
    hub.clients.fetch_sub(1, Ordering::Relaxed);
}

fn channel_name(c: Channel) -> &'static str {
    match c {
        Channel::Swaps => "swaps",
        Channel::Events => "events",
        Channel::Pairs => "pairs",
    }
}
