//! The app API ([`bosphorus_app::App`]) over a C ABI, for a Basecamp core module (C++) that loads
//! Bosphorus in-process. Same shape as `lez-faucet-ffi`: an opaque handle, JSON strings in and
//! out, every returned string freed once with [`bos_app_string_free`], no panic across the
//! boundary, and no function pointers (job events are polled).
//!
//! Every result is `bos-app`'s envelope, `{"id", "result"}` or `{"id", "error": {"code",
//! "message"}}`; `id` is null except in [`bos_app_call`]'s answer. Codes added here: `not_open` (a
//! null handle) and `internal_error` (a panic, stopped at the boundary).
//!
//! Every function but [`bos_app_string_free`] can block (the network; [`bos_app_close`] for the
//! rest of a running proof): call them from a worker thread, never the UI thread. Calls on one
//! handle may overlap from several threads; none may be in flight when it is closed.

use std::{
    ffi::{CStr, CString, c_char},
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    ptr,
    time::Duration,
};

use bosphorus_app::{ApiError, App, Config};
use bosphorus_client::Holder;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    runtime::Runtime,
    sync::broadcast::{
        Receiver,
        error::{RecvError, TryRecvError},
    },
};

/// Opaque handle owned by the caller: one [`App`] and the runtime it runs on.
pub struct BosApp {
    // Dropped in this order: the runtime last, waiting for a running proof; the app's locks go
    // with its last task.
    app: App,
    events: tokio::sync::Mutex<Receiver<Value>>,
    runtime: Runtime,
}

#[repr(C)]
pub struct BosAppOpenOutput {
    /// Null unless the open succeeded. Close it with [`bos_app_close`].
    pub app: *mut BosApp,
    /// `{"id": null, "result": {"wallet_home", "data_dir", "prover"}}` or `{"id": null,
    /// "error"}`. Free it with [`bos_app_string_free`].
    pub result_json: *mut c_char,
}

/// The open config, as JSON.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenConfig {
    /// The wallet's directory (`wallet_config.json`, `storage.json`).
    wallet_home: PathBuf,
    /// The lending program, `Public/<id>`.
    lending: String,
    /// Jobs and caches [default: `<wallet_home>/bosphorus-app`].
    #[serde(default)]
    data_dir: Option<PathBuf>,
    /// First block of the market scan (the lending program's deploy block).
    #[serde(default = "first_block")]
    scan_from: u64,
}

fn first_block() -> u64 {
    1
}

fn error(code: &str, message: impl Into<String>) -> Value {
    json!({ "id": null, "error": ApiError { code: code.into(), message: message.into() } })
}

fn c_string(value: &Value) -> *mut c_char {
    // serde_json writes NUL as `\u0000`, so there's none to refuse.
    CString::new(value.to_string()).map_or(ptr::null_mut(), CString::into_raw)
}

/// Runs an FFI body. A panic becomes `internal_error`: unwinding into C is undefined behaviour.
fn guarded(body: impl FnOnce() -> Result<Value, Value>) -> *mut c_char {
    let value = catch_unwind(AssertUnwindSafe(body))
        .unwrap_or_else(|_| Err(error("internal_error", "Bosphorus hit an internal error and stopped the request safely")));
    c_string(&value.unwrap_or_else(|e| e))
}

/// A caller's string: null or non-UTF-8 is a `bad_request`, not undefined behaviour.
unsafe fn required_str<'a>(value: *const c_char, name: &str) -> Result<&'a str, Value> {
    if value.is_null() {
        return Err(error("bad_request", format!("{name} is null")));
    }
    // SAFETY: the caller passes a valid NUL-terminated string.
    unsafe { CStr::from_ptr(value) }.to_str().map_err(|_| error("bad_request", format!("{name} is not UTF-8")))
}

/// Borrows a handle.
unsafe fn live<'a>(app: *mut BosApp) -> Result<&'a BosApp, Value> {
    // SAFETY: the caller passes null or a live handle from `bos_app_open` that isn't being closed.
    unsafe { app.as_ref() }.ok_or_else(|| error("not_open", "no Bosphorus app is open"))
}

fn open(config_json: &str) -> Result<(BosApp, Value), Value> {
    let config: OpenConfig = serde_json::from_str(config_json).map_err(|e| error("bad_request", format!("config: {e}")))?;
    let lending = config.lending.parse::<Holder>().map_err(|e| error("bad_request", format!("lending: {e:#}")))?.id();
    let data_dir = config.data_dir.unwrap_or_else(|| config.wallet_home.join("bosphorus-app"));
    let (keccak_po2, gpu) = bosphorus_client::proving::info();
    let result = json!({
        "id": null,
        "result": {
            "wallet_home": config.wallet_home.display().to_string(),
            "data_dir": data_dir.display().to_string(),
            "prover": { "keccak_po2": keccak_po2, "gpu": gpu },
        },
    });
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("bosphorus")
        .build()
        .map_err(|e| error("failed", format!("starting the runtime: {e}")))?;
    let app = runtime
        .block_on(App::open(Config { wallet_home: Some(config.wallet_home), lending, data_dir, scan_from: config.scan_from }))
        .map_err(|e| json!({ "id": null, "error": ApiError::from_anyhow(&e) }))?;
    let events = tokio::sync::Mutex::new(app.subscribe());
    Ok((BosApp { app, events, runtime }, result))
}

/// Opens the app on a wallet and resumes its unfinished jobs. `config_json`:
/// `{"wallet_home", "lending": "Public/<id>", "data_dir"?, "scan_from"?}`. The wallet must exist
/// (`wallet_config.json`, `storage.json`); `data_dir` defaults to `<wallet_home>/bosphorus-app`.
///
/// The app must be the only writer of that wallet while it is open. It locks the wallet and data
/// dirs (`bosphorus-app.lock`), so a second app on either, here or in another process (`bos-app`),
/// is refused with `wallet_in_use`; `bos` and LEZ's `wallet` CLI don't look.
///
/// Private txs prove in-process with `bosphorus-prover`: on the GPU when the host has a usable one
/// (Apple silicon), else the CPU. It needs no environment variables; `prover` in the result says
/// what it found (`{"keccak_po2", "gpu"}`).
///
/// # Safety
/// `config_json` must be null or a valid NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bos_app_open(config_json: *const c_char) -> BosAppOpenOutput {
    let mut opened = ptr::null_mut();
    let result_json = guarded(|| {
        let (app, result) = open(unsafe { required_str(config_json, "config_json") }?)?;
        opened = Box::into_raw(Box::new(app));
        Ok(result)
    });
    BosAppOpenOutput { app: opened, result_json }
}

/// Creates a wallet before the first [`bos_app_open`]. `config_json`: `{"wallet_home", "sequencer",
/// "mnemonic"?, "sync_from"?}`. Without `mnemonic` it makes a new wallet that starts at the
/// chain's tip and answers `{"id": null, "result": {"mnemonic": "<12 words>"}}`: show it to the
/// user once, it isn't stored anywhere else (the keys are, in plain JSON, as LEZ v0.3.1 does).
/// With `mnemonic` it restores that wallet, to be scanned from `sync_from` (default block 1) by
/// the app's `sync`, and answers `{"mnemonic": null}`. Refuses a `wallet_home` that holds a wallet.
///
/// # Safety
/// `config_json` must be null or a valid NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bos_wallet_create(config_json: *const c_char) -> *mut c_char {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct CreateConfig {
        wallet_home: PathBuf,
        sequencer: String,
        mnemonic: Option<String>,
        sync_from: Option<u64>,
    }
    guarded(|| {
        let config: CreateConfig = serde_json::from_str(unsafe { required_str(config_json, "config_json") }?)
            .map_err(|e| error("bad_request", format!("config: {e}")))?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| error("failed", format!("starting the runtime: {e}")))?;
        let phrase = runtime
            .block_on(bosphorus_client::funds::create_wallet(&config.wallet_home, &config.sequencer, config.mnemonic.as_deref(), config.sync_from))
            .map_err(|e| json!({ "id": null, "error": ApiError::from_anyhow(&e) }))?;
        Ok(json!({ "id": null, "result": { "mnemonic": phrase } }))
    })
}

/// Asks every call in flight on this handle to end now (they answer `stopping`) and stops the
/// job worker; doesn't block. Call it before joining the threads that make calls, then
/// [`bos_app_close`]. A job stopped mid-proof sent nothing and is `interrupted` at the next open; a
/// transfer stopped mid-proof sends nothing.
///
/// # Safety
/// `app` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bos_app_stop(app: *mut BosApp) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: the caller passes null or a live handle.
        if let Some(app) = unsafe { app.as_ref() } {
            app.app.stop();
        }
    }));
}

/// Closes a handle: stops it ([`bos_app_stop`]) and waits at most a few seconds. A proof still
/// running is left to finish in the background and its result dropped (nothing is sent); the
/// wallet and data dir locks are released.
///
/// # Safety
/// `app` must be null, or a live handle from [`bos_app_open`] with no call in flight, never used
/// or closed again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bos_app_close(app: *mut BosApp) {
    if app.is_null() {
        return;
    }
    // SAFETY: the caller closes a live handle exactly once.
    let app = unsafe { Box::from_raw(app) };
    let _ = catch_unwind(AssertUnwindSafe(move || {
        let BosApp { app, events, runtime } = *app;
        app.stop();
        drop(events);
        drop(app);
        runtime.shutdown_timeout(Duration::from_secs(3));
    }));
}

/// One request, `{"id", "method", "params"}` as a `bos-app` line (methods in
/// docs/design/app-api.md), answered with `{"id", "result"}` or `{"id", "error"}`. Blocks until
/// answered; `submit` answers at once with the queued job, `wait` blocks up to its timeout.
///
/// # Safety
/// `app` must be null or a live handle; `request_json` null or a valid NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bos_app_call(app: *mut BosApp, request_json: *const c_char) -> *mut c_char {
    guarded(|| {
        let app = unsafe { live(app) }?;
        let request = unsafe { required_str(request_json, "request_json") }?;
        Ok(app.runtime.block_on(app.app.handle(request)))
    })
}

/// Job events since the last poll, waiting up to `timeout_ms` (at most an hour) for the first:
/// `{"id": null, "result": {"events": [{"event": "job", "job"}, …], "lagged": n}}`, each `job` as
/// the `job` method shows it. `lagged` > 0: events were dropped (the last 256 are kept), so read
/// `jobs` again. 0 doesn't wait. One poller at a time per handle (others wait their turn).
///
/// # Safety
/// `app` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bos_app_poll_events(app: *mut BosApp, timeout_ms: u64) -> *mut c_char {
    guarded(|| {
        let app = unsafe { live(app) }?;
        Ok(app.runtime.block_on(async {
            let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms.min(3_600_000));
            let mut rx = app.events.lock().await;
            let (mut events, mut lagged) = (vec![], 0);
            loop {
                match rx.try_recv() {
                    Ok(job) => events.push(json!({ "event": "job", "job": job })),
                    Err(TryRecvError::Lagged(n)) => lagged += n,
                    Err(TryRecvError::Empty) if events.is_empty() && lagged == 0 => match tokio::time::timeout_at(deadline, rx.recv()).await {
                        Ok(Ok(job)) => events.push(json!({ "event": "job", "job": job })),
                        Ok(Err(RecvError::Lagged(n))) => lagged += n,
                        Ok(Err(RecvError::Closed)) | Err(_) => break,
                    },
                    Err(TryRecvError::Empty | TryRecvError::Closed) => break,
                }
            }
            json!({ "id": null, "result": { "events": events, "lagged": lagged } })
        }))
    })
}

/// Frees a string returned by this library.
///
/// # Safety
/// `value` must be null, or a string from this library, never used or freed again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bos_app_string_free(value: *mut c_char) {
    if !value.is_null() {
        // SAFETY: allocated by `c_string`, freed once.
        drop(unsafe { CString::from_raw(value) });
    }
}
