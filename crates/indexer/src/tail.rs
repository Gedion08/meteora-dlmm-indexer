//! `tail`: run the live pipeline without a database and print decoded output as JSON
//! lines. Optionally records the raw updates for replay-based tests.

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use prost::Message;
use serde_json::json;
use tokio::sync::mpsc;

use crate::assembler::{Assembler, WriterMsg};
use crate::config::StreamConfig;
use crate::health::Health;
use crate::source::{build_sources, Checkpoint, SourceMsg};

pub async fn run(
    stream: StreamConfig,
    duration: Option<Duration>,
    record: Option<PathBuf>,
    verbose: bool,
) -> Result<()> {
    let shutdown = tokio_util::sync::CancellationToken::new();
    {
        let s = shutdown.clone();
        tokio::spawn(async move {
            match duration {
                Some(d) => tokio::select! { _ = tokio::time::sleep(d) => {}, _ = tokio::signal::ctrl_c() => {} },
                None => {
                    let _ = tokio::signal::ctrl_c().await;
                }
            }
            s.cancel();
        });
    }

    let health = Health::new(stream.grpc_endpoints.len(), stream.idle_timeout(), u64::MAX);
    let checkpoint = Checkpoint::new(0);
    let sources = build_sources(&stream, dlmm_decoder::PROGRAM_ID, &checkpoint, &health)?;

    let (src_tx, mut src_rx) = mpsc::channel::<SourceMsg>(50_000);
    let (asm_tx, asm_rx) = mpsc::channel::<SourceMsg>(50_000);
    let (w_tx, mut w_rx) = mpsc::channel::<WriterMsg>(2_048);

    for s in sources {
        tokio::spawn(s.run(src_tx.clone(), shutdown.clone()));
    }
    drop(src_tx);

    // Tee raw updates to the fixture file before they reach the assembler.
    let mut file = match &record {
        Some(p) => Some(std::io::BufWriter::new(std::fs::File::create(p)?)),
        None => None,
    };
    let tee = tokio::spawn(async move {
        let mut n = 0usize;
        while let Some(msg) = src_rx.recv().await {
            if let (Some(f), SourceMsg::Update { update, .. }) = (file.as_mut(), &msg) {
                let _ = f.write_all(&update.encode_length_delimited_to_vec());
                n += 1;
            }
            if asm_tx.send(msg).await.is_err() {
                break;
            }
        }
        if let Some(mut f) = file {
            let _ = f.flush();
        }
        n
    });

    let asm = Assembler::new(0, stream.slot_meta_timeout(), health, w_tx);
    tokio::spawn(asm.run(asm_rx, shutdown.clone()));

    let (mut slots, mut txs, mut ixs, mut evs, mut accs, mut fails) = (0usize, 0, 0, 0, 0, 0);
    let out = std::io::stdout();
    while let Some(msg) = w_rx.recv().await {
        let WriterMsg::Batch { batch: b, .. } = msg else { continue };
        let mut o = out.lock();
        if b.meta.is_some() {
            slots += 1;
        }
        txs += b.txs.len();
        ixs += b.instructions.len();
        evs += b.events.len();
        accs += b.accounts.len();
        fails += b.failures.len();
        let block_time = b.meta.as_ref().and_then(|m| m.block_time);
        for e in &b.events {
            let line = json!({"kind": "event", "slot": e.slot, "block_time": block_time, "sig": e.signature,
                              "name": e.name, "lb_pair": e.lb_pair, "wallet": e.wallet, "data": e.data});
            let _ = writeln!(o, "{line}");
        }
        if verbose {
            for i in &b.instructions {
                let line = json!({"kind": "instruction", "slot": i.slot, "sig": i.signature, "name": i.name,
                                  "invoked_by": i.invoked_by, "lb_pair": i.lb_pair, "args": i.args});
                let _ = writeln!(o, "{line}");
            }
            for a in &b.accounts {
                let line = json!({"kind": "account", "slot": a.slot, "pubkey": a.pubkey, "type": a.account_type,
                                  "lb_pair": a.lb_pair, "trailing_bytes": a.trailing_bytes});
                let _ = writeln!(o, "{line}");
            }
        }
        for f in &b.failures {
            let line = json!({"kind": "decode_failure", "slot": f.slot, "what": f.kind, "sig": f.signature,
                              "pubkey": f.pubkey, "error": f.error});
            let _ = writeln!(o, "{line}");
        }
    }
    let recorded = tee.await.unwrap_or(0);
    eprintln!(
        "summary: slots_with_meta={slots} transactions={txs} instructions={ixs} events={evs} \
         account_updates={accs} decode_failures={fails} recorded_updates={recorded}"
    );
    Ok(())
}
