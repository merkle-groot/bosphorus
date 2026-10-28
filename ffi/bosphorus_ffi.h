#ifndef BOSPHORUS_FFI_H
#define BOSPHORUS_FFI_H

#pragma once

#include <stdarg.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>

/**
 * Opaque handle owned by the caller: one [`App`] and the runtime it runs on.
 */
typedef struct BosApp BosApp;

typedef struct BosAppOpenOutput {
  /**
   * Null unless the open succeeded. Close it with [`bos_app_close`].
   */
  struct BosApp *app;
  /**
   * `{"id": null, "result": {"wallet_home", "data_dir", "prover"}}` or `{"id": null,
   * "error"}`. Free it with [`bos_app_string_free`].
   */
  char *result_json;
} BosAppOpenOutput;

/**
 * Opens the app on a wallet and resumes its unfinished jobs. `config_json`:
 * `{"wallet_home", "lending": "Public/<id>", "data_dir"?, "scan_from"?}`. The wallet must exist
 * (`wallet_config.json`, `storage.json`); `data_dir` defaults to `<wallet_home>/bosphorus-app`.
 *
 * The app must be the only writer of that wallet while it is open. It locks the wallet and data
 * dirs (`bosphorus-app.lock`), so a second app on either, here or in another process (`bos-app`),
 * is refused with `wallet_in_use`; `bos` and LEZ's `wallet` CLI don't look.
 *
 * Private txs prove in-process with `bosphorus-prover`: on the GPU when the host has a usable one
 * (Apple silicon), else the CPU. It needs no environment variables; `prover` in the result says
 * what it found (`{"keccak_po2", "gpu"}`).
 *
 * # Safety
 * `config_json` must be null or a valid NUL-terminated string.
 */
struct BosAppOpenOutput bos_app_open(const char *config_json);

/**
 * Creates a wallet before the first [`bos_app_open`]. `config_json`: `{"wallet_home", "sequencer",
 * "mnemonic"?, "sync_from"?}`. Without `mnemonic` it makes a new wallet that starts at the
 * chain's tip and answers `{"id": null, "result": {"mnemonic": "<12 words>"}}`: show it to the
 * user once, it isn't stored anywhere else (the keys are, in plain JSON, as LEZ v0.3.1 does).
 * With `mnemonic` it restores that wallet, to be scanned from `sync_from` (default block 1) by
 * the app's `sync`, and answers `{"mnemonic": null}`. Refuses a `wallet_home` that holds a wallet.
 *
 * # Safety
 * `config_json` must be null or a valid NUL-terminated string.
 */
char *bos_wallet_create(const char *config_json);

/**
 * Asks every call in flight on this handle to end now (they answer `stopping`) and stops the
 * job worker; doesn't block. Call it before joining the threads that make calls, then
 * [`bos_app_close`]. A job stopped mid-proof sent nothing and is `interrupted` at the next open; a
 * transfer stopped mid-proof sends nothing.
 *
 * # Safety
 * `app` must be null or a live handle.
 */
void bos_app_stop(struct BosApp *app);

/**
 * Closes a handle: stops it ([`bos_app_stop`]) and waits at most a few seconds. A proof still
 * running is left to finish in the background and its result dropped (nothing is sent); the
 * wallet and data dir locks are released.
 *
 * # Safety
 * `app` must be null, or a live handle from [`bos_app_open`] with no call in flight, never used
 * or closed again.
 */
void bos_app_close(struct BosApp *app);

/**
 * One request, `{"id", "method", "params"}` as a `bos-app` line (methods in
 * docs/design/app-api.md), answered with `{"id", "result"}` or `{"id", "error"}`. Blocks until
 * answered; `submit` answers at once with the queued job, `wait` blocks up to its timeout.
 *
 * # Safety
 * `app` must be null or a live handle; `request_json` null or a valid NUL-terminated string.
 */
char *bos_app_call(struct BosApp *app, const char *request_json);

/**
 * Job events since the last poll, waiting up to `timeout_ms` (at most an hour) for the first:
 * `{"id": null, "result": {"events": [{"event": "job", "job"}, …], "lagged": n}}`, each `job` as
 * the `job` method shows it. `lagged` > 0: events were dropped (the last 256 are kept), so read
 * `jobs` again. 0 doesn't wait. One poller at a time per handle (others wait their turn).
 *
 * # Safety
 * `app` must be null or a live handle.
 */
char *bos_app_poll_events(struct BosApp *app, uint64_t timeout_ms);

/**
 * Frees a string returned by this library.
 *
 * # Safety
 * `value` must be null, or a string from this library, never used or freed again.
 */
void bos_app_string_free(char *value);

#endif  /* BOSPHORUS_FFI_H */
