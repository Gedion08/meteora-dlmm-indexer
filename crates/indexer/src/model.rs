//! Rows produced by the pipeline and written by the store.

use serde_json::Value;

#[derive(Debug, Clone)]
pub struct TxRow {
    pub slot: u64,
    pub signature: String,
    pub tx_index: u64,
    pub fee_payer: String,
    pub success: bool,
    pub err: Option<String>,
    pub fee: u64,
    pub compute_units: Option<u64>,
    pub account_keys: Vec<String>,
    pub pre_token_balances: Value,
    pub post_token_balances: Value,
}

#[derive(Debug, Clone)]
pub struct IxRow {
    pub slot: u64,
    pub signature: String,
    pub tx_index: u64,
    /// Index of the top-level instruction.
    pub ix_index: u16,
    /// Position inside that instruction's inner instructions; -1 for the top-level one.
    pub inner_index: i16,
    pub stack_height: Option<u32>,
    /// Program of the top-level instruction when this one is a CPI (e.g. Jupiter).
    pub invoked_by: Option<String>,
    pub name: &'static str,
    pub lb_pair: Option<String>,
    pub wallet: Option<String>,
    pub accounts: Value,
    pub remaining_accounts: Vec<String>,
    pub args: Value,
    pub success: bool,
}

#[derive(Debug, Clone)]
pub struct EventRow {
    pub slot: u64,
    pub signature: String,
    pub tx_index: u64,
    pub ix_index: u16,
    pub inner_index: i16,
    pub name: &'static str,
    pub lb_pair: Option<String>,
    pub position: Option<String>,
    pub wallet: Option<String>,
    pub data: Value,
}

#[derive(Debug, Clone)]
pub struct AccountRow {
    pub pubkey: String,
    pub account_type: &'static str,
    pub slot: u64,
    pub write_version: u64,
    pub lamports: u64,
    pub data_len: usize,
    pub trailing_bytes: usize,
    pub lb_pair: Option<String>,
    pub owner_wallet: Option<String>,
    pub data: Value,
}

/// An account that went to zero lamports in a successful DLMM transaction.
#[derive(Debug, Clone)]
pub struct AccountClose {
    pub pubkey: String,
    pub slot: u64,
}

#[derive(Debug, Clone)]
pub struct DecodeFailure {
    pub slot: u64,
    pub kind: &'static str,
    pub signature: Option<String>,
    pub pubkey: Option<String>,
    pub ix_index: Option<u16>,
    pub inner_index: Option<i16>,
    pub raw: Vec<u8>,
    pub error: String,
}

#[derive(Debug, Clone, Default)]
pub struct SlotMeta {
    pub parent_slot: Option<u64>,
    pub block_time: Option<i64>,
    pub block_height: Option<u64>,
    pub blockhash: Option<String>,
}

/// Everything decoded for one slot (or late-arriving data for an already-emitted slot).
#[derive(Debug, Default)]
pub struct SlotBatch {
    pub slot: u64,
    pub meta: Option<SlotMeta>,
    /// Block time arrived after rows for this slot were already written without it.
    pub backfill_block_time: bool,
    pub txs: Vec<TxRow>,
    pub instructions: Vec<IxRow>,
    pub events: Vec<EventRow>,
    pub accounts: Vec<AccountRow>,
    pub closes: Vec<AccountClose>,
    pub failures: Vec<DecodeFailure>,
}

impl SlotBatch {
    pub fn new(slot: u64) -> Self {
        Self {
            slot,
            ..Default::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.meta.is_none()
            && self.txs.is_empty()
            && self.instructions.is_empty()
            && self.events.is_empty()
            && self.accounts.is_empty()
            && self.closes.is_empty()
            && self.failures.is_empty()
    }
}

/// Pick the first present string field from a decoded JSON object.
pub fn first_str(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str))
        .map(str::to_owned)
}

pub const WALLET_EVENT_FIELDS: &[&str] = &["from", "owner", "sender", "funder"];
pub const WALLET_IX_ACCOUNTS: &[&str] = &["user", "sender", "owner", "funder", "signer", "payer"];
