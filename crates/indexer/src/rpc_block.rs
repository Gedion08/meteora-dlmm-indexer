//! Converts JSON-RPC `getBlock` transactions (encoding "json", any version: legacy, v0,
//! v1) into the Yellowstone protobuf shape, so RPC-fetched data flows through exactly the
//! same extractor as the live stream.

use serde_json::Value;
use yellowstone_grpc_proto::prelude::{
    CompiledInstruction, InnerInstruction, InnerInstructions, Message, MessageHeader,
    SubscribeUpdateTransactionInfo, TokenBalance, Transaction, TransactionError,
    TransactionStatusMeta, UiTokenAmount,
};

fn b58(v: &Value) -> Option<Vec<u8>> {
    bs58::decode(v.as_str()?).into_vec().ok()
}

fn b58_list(v: &Value) -> Vec<Vec<u8>> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(b58)
        .collect()
}

fn u8_list(v: &Value) -> Vec<u8> {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| x.as_u64().map(|n| n as u8))
        .collect()
}

fn u64_list(v: &Value) -> Vec<u64> {
    v.as_array().into_iter().flatten().filter_map(Value::as_u64).collect()
}

fn token_balances(v: &Value) -> Vec<TokenBalance> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|b| TokenBalance {
            account_index: b["accountIndex"].as_u64().unwrap_or_default() as u32,
            mint: b["mint"].as_str().unwrap_or_default().to_owned(),
            owner: b["owner"].as_str().unwrap_or_default().to_owned(),
            program_id: b["programId"].as_str().unwrap_or_default().to_owned(),
            ui_token_amount: Some(UiTokenAmount {
                ui_amount: b["uiTokenAmount"]["uiAmount"].as_f64().unwrap_or_default(),
                decimals: b["uiTokenAmount"]["decimals"].as_u64().unwrap_or_default() as u32,
                amount: b["uiTokenAmount"]["amount"].as_str().unwrap_or_default().to_owned(),
                ui_amount_string: b["uiTokenAmount"]["uiAmountString"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            }),
        })
        .collect()
}

/// `index` is the transaction's position in the block.
pub fn tx_from_json(tx: &Value, index: u64) -> Option<SubscribeUpdateTransactionInfo> {
    let t = &tx["transaction"];
    let m = &t["message"];
    let meta = &tx["meta"];
    let signatures = b58_list(&t["signatures"]);
    let header = &m["header"];

    let message = Message {
        header: Some(MessageHeader {
            num_required_signatures: header["numRequiredSignatures"].as_u64()? as u32,
            num_readonly_signed_accounts: header["numReadonlySignedAccounts"].as_u64()? as u32,
            num_readonly_unsigned_accounts: header["numReadonlyUnsignedAccounts"].as_u64()? as u32,
        }),
        account_keys: b58_list(&m["accountKeys"]),
        recent_blockhash: b58(&m["recentBlockhash"]).unwrap_or_default(),
        instructions: m["instructions"]
            .as_array()?
            .iter()
            .map(|ix| CompiledInstruction {
                program_id_index: ix["programIdIndex"].as_u64().unwrap_or_default() as u32,
                accounts: u8_list(&ix["accounts"]),
                data: b58(&ix["data"]).unwrap_or_default(),
            })
            .collect(),
        versioned: !tx["version"].is_string(), // "legacy" vs numeric versions
        ..Default::default()
    };

    let inner_instructions = meta["innerInstructions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|g| InnerInstructions {
            index: g["index"].as_u64().unwrap_or_default() as u32,
            instructions: g["instructions"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|ix| InnerInstruction {
                    program_id_index: ix["programIdIndex"].as_u64().unwrap_or_default() as u32,
                    accounts: u8_list(&ix["accounts"]),
                    data: b58(&ix["data"]).unwrap_or_default(),
                    stack_height: ix["stackHeight"].as_u64().map(|h| h as u32),
                })
                .collect(),
        })
        .collect();

    let meta = TransactionStatusMeta {
        // The proto carries a bincode blob; the extractor reads the reason from logs.
        err: (!meta["err"].is_null()).then(|| TransactionError {
            err: meta["err"].to_string().into_bytes(),
        }),
        fee: meta["fee"].as_u64().unwrap_or_default(),
        pre_balances: u64_list(&meta["preBalances"]),
        post_balances: u64_list(&meta["postBalances"]),
        inner_instructions,
        log_messages: meta["logMessages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|l| l.as_str().map(str::to_owned))
            .collect(),
        pre_token_balances: token_balances(&meta["preTokenBalances"]),
        post_token_balances: token_balances(&meta["postTokenBalances"]),
        loaded_writable_addresses: b58_list(&meta["loadedAddresses"]["writable"]),
        loaded_readonly_addresses: b58_list(&meta["loadedAddresses"]["readonly"]),
        compute_units_consumed: meta["computeUnitsConsumed"].as_u64(),
        ..Default::default()
    };

    Some(SubscribeUpdateTransactionInfo {
        signature: signatures.first()?.clone(),
        is_vote: false,
        transaction: Some(Transaction {
            signatures,
            message: Some(message),
        }),
        meta: Some(meta),
        index,
    })
}

/// Cheap pre-filter: does the transaction reference `program` at all?
pub fn mentions(tx: &Value, program: &str) -> bool {
    let has = |v: &Value| v.as_array().is_some_and(|a| a.iter().any(|k| k.as_str() == Some(program)));
    has(&tx["transaction"]["message"]["accountKeys"])
        || has(&tx["meta"]["loadedAddresses"]["writable"])
        || has(&tx["meta"]["loadedAddresses"]["readonly"])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::extract_transaction;
    use crate::model::SlotBatch;
    use dlmm_decoder::Decoder;

    /// RPC-converted mainnet transactions (legacy, v0, v1) decode without failures.
    #[test]
    fn converts_mainnet_rpc_transactions() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/transactions/recent.json");
        let txs: Vec<Value> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let dec = Decoder::bundled();
        let mut batch = SlotBatch::new(0);
        let mut invoked = 0;
        for (i, tx) in txs.iter().enumerate() {
            assert!(mentions(tx, dlmm_decoder::PROGRAM_ID));
            let info = tx_from_json(tx, i as u64).expect("convertible");
            invoked += extract_transaction(dec, tx["slot"].as_u64().unwrap(), &info, &mut batch) as usize;
        }
        assert!(batch.failures.is_empty(), "{:?}", batch.failures);
        assert!(invoked > 0 && !batch.events.is_empty() && !batch.instructions.is_empty());
        // Token balances survive the conversion (needed for mints and reserves).
        assert!(batch.txs.iter().any(|t| t.post_token_balances.as_array().is_some_and(|a| !a.is_empty())));
    }
}
