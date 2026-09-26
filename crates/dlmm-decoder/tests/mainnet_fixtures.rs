//! Golden tests against real mainnet data in `fixtures/` (accounts via
//! getProgramAccountsV2, transactions via getTransaction — legacy, v0 and v1).

use std::collections::BTreeMap;
use std::path::PathBuf;

use base64::Engine;
use dlmm_decoder::Decoder;
use serde_json::Value;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

/// Types whose on-chain size is exactly the IDL layout (no dynamic tail).
const FIXED_SIZE: &[&str] = &[
    "LbPair",
    "BinArray",
    "BinArrayBitmapExtension",
    "ClaimFeeOperator",
    "Operator",
    "PresetParameter2",
    "TokenBadge",
];

#[test]
fn decodes_every_account_fixture() {
    let dec = Decoder::bundled();
    let dir = fixtures().join("accounts");
    let mut seen = 0;
    for entry in std::fs::read_dir(&dir).expect("fixtures/accounts exists") {
        let path = entry.unwrap().path();
        let fx: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let data = base64::engine::general_purpose::STANDARD
            .decode(fx["data_base64"].as_str().unwrap())
            .unwrap();
        let expected = path.file_stem().unwrap().to_str().unwrap();

        let acc = dec
            .decode_account(&data)
            .unwrap_or_else(|e| panic!("{expected}: {e}"));
        assert_eq!(acc.name, expected);
        assert_eq!(dec.account_type(&data), Some(expected));
        if FIXED_SIZE.contains(&expected) {
            assert_eq!(acc.trailing_bytes, 0, "{expected}: layout size mismatch");
        }
        seen += 1;
    }
    assert!(seen >= 8, "expected account fixtures, found {seen}");
}

#[test]
fn lb_pair_fields_are_sane() {
    let dec = Decoder::bundled();
    let fx: Value =
        serde_json::from_slice(&std::fs::read(fixtures().join("accounts/LbPair.json")).unwrap()).unwrap();
    let data = base64::engine::general_purpose::STANDARD
        .decode(fx["data_base64"].as_str().unwrap())
        .unwrap();
    let pair = dec.decode_account(&data).unwrap().data;

    let bin_step = pair["bin_step"].as_u64().unwrap();
    assert!((1..=400).contains(&bin_step), "bin_step {bin_step}");
    for k in ["token_x_mint", "token_y_mint", "reserve_x", "reserve_y", "oracle"] {
        let s = pair[k].as_str().unwrap();
        assert!((32..=44).contains(&s.len()), "{k} = {s}");
    }
    assert_eq!(pair["reward_infos"].as_array().unwrap().len(), 2);
    // Padding is decoded but not emitted.
    assert!(pair.get("_padding_1").is_none() && pair.get("_reserved").is_none());
}

#[test]
fn bin_array_has_70_bins_with_u128_strings() {
    let dec = Decoder::bundled();
    let fx: Value =
        serde_json::from_slice(&std::fs::read(fixtures().join("accounts/BinArray.json")).unwrap()).unwrap();
    let data = base64::engine::general_purpose::STANDARD
        .decode(fx["data_base64"].as_str().unwrap())
        .unwrap();
    let arr = dec.decode_account(&data).unwrap().data;
    let bins = arr["bins"].as_array().unwrap();
    assert_eq!(bins.len(), 70);
    // u128 values are decimal strings so they survive JSON round-trips losslessly.
    assert!(bins[0]["price"].as_str().unwrap().parse::<u128>().is_ok());
    assert!(bins[0]["amount_x"].is_u64());
}

/// Walk every DLMM instruction (top-level and CPI) in the transaction fixtures.
#[test]
fn decodes_all_instructions_and_events_in_recent_transactions() {
    let dec = Decoder::bundled();
    let txs: Vec<Value> = serde_json::from_slice(
        &std::fs::read(fixtures().join("transactions/recent.json")).expect("transaction fixtures"),
    )
    .unwrap();
    assert!(!txs.is_empty());

    let mut ix_counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut ev_counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut failures = Vec::new();

    for tx in &txs {
        let msg = &tx["transaction"]["message"];
        let meta = &tx["meta"];
        let sig = tx["transaction"]["signatures"][0].as_str().unwrap();
        let keys: Vec<Vec<u8>> = msg["accountKeys"]
            .as_array()
            .unwrap()
            .iter()
            .chain(meta["loadedAddresses"]["writable"].as_array().into_iter().flatten())
            .chain(meta["loadedAddresses"]["readonly"].as_array().into_iter().flatten())
            .map(|k| bs58::decode(k.as_str().unwrap()).into_vec().unwrap())
            .collect();

        let mut all = Vec::new();
        for ix in msg["instructions"].as_array().unwrap() {
            all.push(ix.clone());
        }
        for group in meta["innerInstructions"].as_array().into_iter().flatten() {
            for ix in group["instructions"].as_array().unwrap() {
                all.push(ix.clone());
            }
        }

        for ix in all {
            let prog = &keys[ix["programIdIndex"].as_u64().unwrap() as usize];
            if prog.as_slice() != dec.program_id() {
                continue;
            }
            let data = bs58::decode(ix["data"].as_str().unwrap()).into_vec().unwrap();
            let accounts: Vec<&[u8]> = ix["accounts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| keys[i.as_u64().unwrap() as usize].as_slice())
                .collect();
            if Decoder::is_event_cpi(&data) {
                match dec.decode_event_cpi(&data) {
                    Ok(e) => *ev_counts.entry(e.name).or_default() += 1,
                    Err(e) => failures.push(format!("{sig} event: {e}")),
                }
            } else {
                match dec.decode_instruction(&data, &accounts) {
                    Ok(d) => {
                        assert_eq!(d.trailing_bytes, 0, "{sig} {}: trailing bytes", d.name);
                        if d.accounts.get("lb_pair").is_some() {
                            assert!(d.accounts["lb_pair"].is_string(), "{sig} {}", d.name);
                        }
                        *ix_counts.entry(d.name).or_default() += 1;
                    }
                    Err(e) => failures.push(format!("{sig} instruction: {e}")),
                }
            }
        }
    }

    eprintln!("instructions: {ix_counts:?}");
    eprintln!("events: {ev_counts:?}");
    assert!(failures.is_empty(), "decode failures:\n{}", failures.join("\n"));
    assert!(!ix_counts.is_empty(), "no DLMM instructions found in fixtures");
    assert!(!ev_counts.is_empty(), "no DLMM events found in fixtures");
}

#[test]
fn discriminator_lookup_and_unknown_data() {
    let dec = Decoder::bundled();
    assert_eq!(dec.program_id_str(), dlmm_decoder::PROGRAM_ID);
    let disc = dec.account_discriminator("LbPair").unwrap();
    assert_eq!(dec.account_type(&disc), Some("LbPair"));
    assert!(dec.decode_instruction(&[0xff; 16], &[]).is_err());
    assert!(dec.decode_account(&[1, 2, 3]).is_err());
}
