//! Turns raw Yellowstone transaction / account updates into rows.

use std::collections::HashSet;

use dlmm_decoder::{DecodeError, Decoder};
use serde_json::{json, Value};
use yellowstone_grpc_proto::prelude::{
    SubscribeUpdateAccountInfo, SubscribeUpdateTransactionInfo, TokenBalance,
};

use crate::model::*;

fn b58(b: &[u8]) -> String {
    bs58::encode(b).into_string()
}

fn failure(
    slot: u64,
    kind: &'static str,
    signature: &str,
    ix: (u16, i16),
    raw: &[u8],
    e: DecodeError,
) -> DecodeFailure {
    metrics::counter!("decode_failures_total", "kind" => kind).increment(1);
    DecodeFailure {
        slot,
        kind,
        signature: Some(signature.to_owned()),
        pubkey: None,
        ix_index: Some(ix.0),
        inner_index: Some(ix.1),
        raw: raw.to_vec(),
        error: e.to_string(),
    }
}

fn token_balances(list: &[TokenBalance], keys: &[&[u8]]) -> Value {
    Value::Array(
        list.iter()
            .map(|b| {
                let amt = b.ui_token_amount.as_ref();
                json!({
                    "account_index": b.account_index,
                    "account": keys.get(b.account_index as usize).map(|k| b58(k)),
                    "mint": b.mint,
                    "owner": b.owner,
                    "program_id": b.program_id,
                    "amount": amt.map(|a| a.amount.clone()),
                    "decimals": amt.map(|a| a.decimals),
                })
            })
            .collect(),
    )
}

/// Human-readable failure reason from the logs (the proto carries a bincode blob).
fn error_from_logs(logs: &[String]) -> Option<String> {
    logs.iter()
        .rev()
        .find(|l| l.contains(" failed: ") || l.starts_with("Program log: AnchorError"))
        .cloned()
}

/// Decode one transaction into `out`. Returns false if it contained nothing from DLMM
/// (e.g. it only referenced the program ID as a read-only account).
pub fn extract_transaction(
    dec: &Decoder,
    slot: u64,
    info: &SubscribeUpdateTransactionInfo,
    out: &mut SlotBatch,
) -> bool {
    let (Some(tx), Some(meta)) = (&info.transaction, &info.meta) else {
        return false;
    };
    let Some(msg) = &tx.message else { return false };

    // Static keys, then ALT-loaded writable, then ALT-loaded readonly (runtime order).
    let keys: Vec<&[u8]> = msg
        .account_keys
        .iter()
        .chain(&meta.loaded_writable_addresses)
        .chain(&meta.loaded_readonly_addresses)
        .map(Vec::as_slice)
        .collect();
    let program = dec.program_id().as_slice();
    let signature = b58(&info.signature);
    let success = meta.err.is_none();
    let key = |i: u32| keys.get(i as usize).copied();

    let mut found = false;
    let mut touched: HashSet<u8> = HashSet::new();

    let mut decode_ix = |out: &mut SlotBatch,
                         data: &[u8],
                         acc_idx: &[u8],
                         ix_index: u16,
                         inner_index: i16,
                         stack_height: Option<u32>,
                         invoked_by: Option<String>| {
        found = true;
        touched.extend(acc_idx.iter().copied());
        if Decoder::is_event_cpi(data) {
            match dec.decode_event_cpi(data) {
                Ok(ev) => {
                    metrics::counter!("events_total", "name" => ev.name).increment(1);
                    out.events.push(EventRow {
                        slot,
                        signature: signature.clone(),
                        tx_index: info.index,
                        ix_index,
                        inner_index,
                        name: ev.name,
                        lb_pair: first_str(&ev.data, &["lb_pair"]),
                        position: first_str(&ev.data, &["position"]),
                        wallet: first_str(&ev.data, WALLET_EVENT_FIELDS),
                        data: ev.data,
                    });
                }
                Err(e) => out
                    .failures
                    .push(failure(slot, "event", &signature, (ix_index, inner_index), data, e)),
            }
            return;
        }
        let accounts: Vec<&[u8]> = acc_idx.iter().filter_map(|i| key(*i as u32)).collect();
        match dec.decode_instruction(data, &accounts) {
            Ok(ix) => {
                metrics::counter!("instructions_total", "name" => ix.name).increment(1);
                out.instructions.push(IxRow {
                    slot,
                    signature: signature.clone(),
                    tx_index: info.index,
                    ix_index,
                    inner_index,
                    stack_height,
                    invoked_by,
                    name: ix.name,
                    lb_pair: first_str(&ix.accounts, &["lb_pair"]),
                    wallet: first_str(&ix.accounts, WALLET_IX_ACCOUNTS),
                    accounts: ix.accounts,
                    remaining_accounts: ix.remaining_accounts,
                    args: ix.args,
                    success,
                });
            }
            Err(e) => out.failures.push(failure(
                slot,
                "instruction",
                &signature,
                (ix_index, inner_index),
                data,
                e,
            )),
        }
    };

    for (i, ix) in msg.instructions.iter().enumerate() {
        let i = i as u16;
        let top_program = key(ix.program_id_index);
        if top_program == Some(program) {
            decode_ix(out, &ix.data, &ix.accounts, i, -1, Some(1), None);
        }
        let Some(inner) = meta.inner_instructions.iter().find(|x| x.index == i as u32) else {
            continue;
        };
        for (j, inn) in inner.instructions.iter().enumerate() {
            if key(inn.program_id_index) != Some(program) {
                continue;
            }
            decode_ix(
                out,
                &inn.data,
                &inn.accounts,
                i,
                j as i16,
                inn.stack_height,
                top_program.map(b58),
            );
        }
    }

    if !found {
        return false;
    }

    // Accounts DLMM touched that were drained to zero lamports were closed
    // (positions, bin arrays, limit orders, ...). Their final update never matches the
    // owner filter because closing reassigns them to the system program.
    if success {
        for &i in &touched {
            let i = i as usize;
            let pre = meta.pre_balances.get(i).copied().unwrap_or(0);
            let post = meta.post_balances.get(i).copied().unwrap_or(0);
            if pre > 0 && post == 0 {
                if let Some(k) = keys.get(i) {
                    out.closes.push(AccountClose { pubkey: b58(k), slot });
                }
            }
        }
    }

    out.txs.push(TxRow {
        slot,
        signature,
        tx_index: info.index,
        fee_payer: keys.first().map(|k| b58(k)).unwrap_or_default(),
        success,
        err: meta
            .err
            .as_ref()
            .map(|e| error_from_logs(&meta.log_messages).unwrap_or_else(|| hex::encode(&e.err))),
        fee: meta.fee,
        compute_units: meta.compute_units_consumed,
        account_keys: keys.iter().map(|k| b58(k)).collect(),
        pre_token_balances: token_balances(&meta.pre_token_balances, &keys),
        post_token_balances: token_balances(&meta.post_token_balances, &keys),
    });
    metrics::counter!("transactions_total").increment(1);
    true
}

pub enum AccountOutcome {
    Row(AccountRow),
    Closed(AccountClose),
    Failed(DecodeFailure),
}

pub fn extract_account(dec: &Decoder, slot: u64, info: &SubscribeUpdateAccountInfo) -> AccountOutcome {
    let pubkey = b58(&info.pubkey);
    if info.lamports == 0 || info.data.is_empty() {
        return AccountOutcome::Closed(AccountClose { pubkey, slot });
    }
    match account_row(dec, pubkey, slot, info.write_version, info.lamports, &info.data) {
        Ok(row) => AccountOutcome::Row(row),
        Err(f) => AccountOutcome::Failed(f),
    }
}

/// Shared by the live stream and the RPC snapshot.
pub fn account_row(
    dec: &Decoder,
    pubkey: String,
    slot: u64,
    write_version: u64,
    lamports: u64,
    data: &[u8],
) -> Result<AccountRow, DecodeFailure> {
    match dec.decode_account(data) {
        Ok(acc) => {
            metrics::counter!("account_updates_total", "type" => acc.name).increment(1);
            Ok(AccountRow {
                lb_pair: first_str(&acc.data, &["lb_pair"]),
                owner_wallet: first_str(&acc.data, &["owner"]),
                pubkey,
                account_type: acc.name,
                slot,
                write_version,
                lamports,
                data_len: data.len(),
                trailing_bytes: acc.trailing_bytes,
                data: acc.data,
            })
        }
        Err(e) => {
            metrics::counter!("decode_failures_total", "kind" => "account").increment(1);
            Err(DecodeFailure {
                slot,
                kind: "account",
                signature: None,
                pubkey: Some(pubkey),
                ix_index: None,
                inner_index: None,
                // Keep only the head of large accounts; enough to identify the layout.
                raw: data[..data.len().min(1024)].to_vec(),
                error: e.to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;
    use yellowstone_grpc_proto::geyser::{subscribe_update::UpdateOneof, SubscribeUpdate};

    /// Replays a real LaserStream capture (`tail --record`) through the extractors.
    #[test]
    fn replay_recorded_stream() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/stream-devnet.bin");
        let bytes = std::fs::read(path).expect("recorded stream fixture");
        let mut buf = bytes.as_slice();
        let dec = Decoder::bundled();
        let mut batch = SlotBatch::new(0);
        let (mut txs, mut accounts, mut metas) = (0, 0, 0);
        while !buf.is_empty() {
            let u = SubscribeUpdate::decode_length_delimited(&mut buf).expect("valid frame");
            match u.update_oneof {
                Some(UpdateOneof::Transaction(t)) => {
                    txs += extract_transaction(dec, t.slot, t.transaction.as_ref().unwrap(), &mut batch) as usize;
                }
                Some(UpdateOneof::Account(a)) => match extract_account(dec, a.slot, a.account.as_ref().unwrap()) {
                    AccountOutcome::Row(_) | AccountOutcome::Closed(_) => accounts += 1,
                    AccountOutcome::Failed(f) => panic!("account decode failed: {}", f.error),
                },
                Some(UpdateOneof::BlockMeta(_)) => metas += 1,
                _ => {}
            }
        }
        assert!(batch.failures.is_empty(), "{:?}", batch.failures);
        assert!(txs > 0 && accounts > 0 && metas > 0, "txs={txs} accounts={accounts} metas={metas}");
        assert!(batch.events.iter().any(|e| e.name == "Swap"));
        for ix in &batch.instructions {
            assert!(ix.lb_pair.is_some(), "{} without lb_pair", ix.name);
        }
        // Every swap instruction's events carry the pair and the trader.
        for e in batch.events.iter().filter(|e| e.name.starts_with("Swap")) {
            assert!(e.lb_pair.is_some() && e.wallet.is_some());
        }
    }
}
