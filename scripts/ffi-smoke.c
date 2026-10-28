/*
 * ffi-smoke: the Bosphorus C ABI (ffi/bosphorus_ffi.h) from C, read-only: no tx is sent.
 *
 * Opens the app on the wallet named in argv[1] (bos_app_open's config JSON), checks that a second
 * open of the same wallet is refused, calls `info` and an unknown method, polls events without
 * waiting, closes, and opens again. Null handles and strings must come back as errors, not
 * crashes. Built and run by scripts/ffi-smoke.sh.
 */
#include <stdio.h>
#include <string.h>

#include "bosphorus_ffi.h"

static int failures = 0;

/* Prints and frees a string from the library; whether it contains `want`. */
static int expect(const char *what, char *json, const char *want)
{
    int ok = json != NULL && strstr(json, want) != NULL;
    printf("%s %s: %s\n", ok ? "ok  " : "FAIL", what, json != NULL ? json : "(null)");
    if (!ok) {
        failures++;
    }
    bos_app_string_free(json);
    return ok;
}

int main(int argc, char **argv)
{
    if (argc != 2) {
        fprintf(stderr, "usage: ffi-smoke '<bos_app_open config JSON>'\n");
        return 2;
    }

    BosAppOpenOutput bad = bos_app_open(NULL);
    expect("open(NULL) refused", bad.result_json, "\"bad_request\"");
    if (bad.app != NULL) {
        failures++;
    }

    BosAppOpenOutput out = bos_app_open(argv[1]);
    if (!expect("open", out.result_json, "\"result\"") || out.app == NULL) {
        return 1;
    }
    BosApp *app = out.app;

    BosAppOpenOutput twice = bos_app_open(argv[1]);
    expect("second open of the same wallet refused", twice.result_json, "\"wallet_in_use\"");
    if (twice.app != NULL) {
        failures++;
        bos_app_close(twice.app);
    }

    expect("info", bos_app_call(app, "{\"id\":1,\"method\":\"info\"}"), "\"last_block\"");
    expect("unknown method", bos_app_call(app, "{\"id\":2,\"method\":\"nope\"}"), "\"unknown_method\"");
    expect("not JSON", bos_app_call(app, "{"), "\"bad_request\"");
    expect("null request", bos_app_call(app, NULL), "\"bad_request\"");
    expect("null handle", bos_app_call(NULL, "{\"id\":3,\"method\":\"info\"}"), "\"not_open\"");
    expect("events, no wait", bos_app_poll_events(app, 0), "\"events\"");

    bos_app_close(app);
    bos_app_close(NULL);
    bos_app_string_free(NULL);

    /* Closed, so the wallet is free again. */
    BosAppOpenOutput again = bos_app_open(argv[1]);
    expect("open again after close", again.result_json, "\"result\"");
    bos_app_close(again.app);

    if (failures != 0) {
        printf("ffi-smoke: %d failed\n", failures);
        return 1;
    }
    printf("ffi-smoke: all ok\n");
    return 0;
}
