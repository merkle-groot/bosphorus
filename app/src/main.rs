//! `bos-app`: the app API as JSON lines on stdin/stdout. One request per line
//! (`{"id": 1, "method": "markets", "params": {}}`), one response per line, plus a
//! `{"event": "job", "job": {…}}` line at every job change. Requests run concurrently. Uses the
//! wallet at `LEE_WALLET_HOME_DIR`; private txs need real proofs, or `RISC0_DEV_MODE=1` against a
//! dev-mode sequencer.
//!
//! stdout carries only the protocol: the LEZ wallet prints to stdout, so fd 1 is pointed at
//! stderr and the protocol gets its own copy of the original stdout.

use std::{io::Write as _, os::fd::FromRawFd as _, path::PathBuf};

use anyhow::{Context as _, Result, ensure};
use bosphorus_app::{App, Config};
use bosphorus_client::Holder;
use clap::Parser;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt as _, BufReader},
    sync::{broadcast::error::RecvError, mpsc},
    task::JoinSet,
};

#[derive(Parser)]
#[command(name = "bos-app", about = "Bosphorus app API: JSON lines on stdin/stdout")]
struct Args {
    #[arg(long)]
    lending: Holder,
    /// First block of the market scan (the lending program's deploy block).
    #[arg(long, default_value_t = 1)]
    scan_from: u64,
    /// Jobs and caches [default: $LEE_WALLET_HOME_DIR/bosphorus-app].
    #[arg(long)]
    data_dir: Option<PathBuf>,
}

/// The original stdout, for the protocol; fd 1 now goes to stderr.
fn protocol_stdout() -> Result<std::fs::File> {
    // SAFETY: dup/dup2 on the process's own standard descriptors, before anything writes to them;
    // the duplicate is owned by the returned file alone.
    unsafe {
        let fd = libc::dup(1);
        ensure!(fd >= 0, "dup(stdout) failed");
        ensure!(libc::dup2(2, 1) >= 0, "dup2(stderr, stdout) failed");
        Ok(std::fs::File::from_raw_fd(fd))
    }
}

fn main() -> Result<()> {
    // SAFETY: no runtime or worker threads exist yet.
    unsafe { bosphorus_client::proving::configure() };
    tokio::runtime::Runtime::new()?.block_on(run())
}

async fn run() -> Result<()> {
    let args = Args::parse();
    let mut out = protocol_stdout()?;
    let data_dir = match args.data_dir {
        Some(dir) => dir,
        None => PathBuf::from(std::env::var_os("LEE_WALLET_HOME_DIR").context("set --data-dir or LEE_WALLET_HOME_DIR")?).join("bosphorus-app"),
    };
    let app = App::open(Config { wallet_home: None, lending: args.lending.id(), data_dir, scan_from: args.scan_from }).await?;

    let (lines_tx, mut lines_rx) = mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        while let Some(line) = lines_rx.recv().await {
            writeln!(out, "{line}")?;
            out.flush()?;
        }
        anyhow::Ok(())
    });
    let mut events = app.subscribe();
    let events_tx = lines_tx.clone();
    let events_task = tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(job) => {
                    let _ = events_tx.send(json!({ "event": "job", "job": job }));
                }
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            }
        }
    });

    let mut stdin = BufReader::new(tokio::io::stdin()).lines();
    let mut pending = JoinSet::new();
    while let Some(line) = stdin.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let (app, tx) = (app.clone(), lines_tx.clone());
        pending.spawn(async move {
            let _ = tx.send(app.handle(&line).await);
        });
    }
    // stdin closed: answer what's pending, then stop (unfinished jobs resume on the next start).
    while pending.join_next().await.is_some() {}
    events_task.abort();
    let _ = events_task.await;
    drop(lines_tx);
    writer.await?
}
