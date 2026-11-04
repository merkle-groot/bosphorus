# A newcomer's path over the C ABI, as the Basecamp module drives it: a fresh wallet with no
# native claims its first gas with its private account (fee-exempt), then USDC and WETH with its
# public one, adds WETH as security and borrows USDC; then dives WETH and borrows privately.
#   python3 basecamp/dev/onboard_flow.py <settings.json from scripts/onboard-deploy.sh> [--public-only]
import ctypes, json, os, shutil, sys, time
lib = ctypes.CDLL(os.path.expanduser("~/Work/money-market/target/release/libbosphorus_ffi.dylib"))
lib.bos_wallet_create.restype = ctypes.c_void_p; lib.bos_wallet_create.argtypes = [ctypes.c_char_p]
class Open(ctypes.Structure): _fields_ = [("app", ctypes.c_void_p), ("result_json", ctypes.c_void_p)]
lib.bos_app_open.restype = Open; lib.bos_app_open.argtypes = [ctypes.c_char_p]
lib.bos_app_call.restype = ctypes.c_void_p; lib.bos_app_call.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
lib.bos_app_string_free.argtypes = [ctypes.c_void_p]; lib.bos_app_close.argtypes = [ctypes.c_void_p]

def take(p):
    s = ctypes.string_at(p).decode(); lib.bos_app_string_free(p); return json.loads(s)

settings = json.load(open(sys.argv[1]))
public_only = "--public-only" in sys.argv
symbol = {v: k for k, v in settings["tokens"].items()}
usdc, weth = symbol["USDC"], symbol["WETH"]
home = "/tmp/bc-onboard/wallet"; shutil.rmtree("/tmp/bc-onboard", ignore_errors=True)
r = take(lib.bos_wallet_create(json.dumps({"wallet_home": home, "sequencer": settings["sequencer"]}).encode()))
assert "result" in r, r
o = lib.bos_app_open(json.dumps({"wallet_home": home, "lending": settings["lending"], "faucet": settings["faucet"], "scan_from": settings["scan_from"]}).encode())
print("open:", take(o.result_json)); app = o.app
n = [0]
def call(method, params={}, ok=True):
    n[0] += 1; t = time.time()
    r = take(lib.bos_app_call(app, json.dumps({"id": n[0], "method": method, "params": params}).encode()))
    print(f"{method} ({time.time() - t:.1f} s):", json.dumps(r.get("result", r.get("error")))[:400], flush=True)
    if ok: assert "result" in r, r
    return r.get("result", r.get("error"))
def native(account):
    return int(next(a for a in call("accounts") if a["account"] == account)["native"] or 0)
def wait_job(job):
    while True:
        j = call("job", {"id": job})
        if j["state"] in ("done", "failed", "interrupted", "unknown"): return j
        time.sleep(3)
def op(**req):
    q = call("quote", req); j = call("submit", req)
    done = wait_job(j["id"]); assert done["state"] == "done", done
    return q

pub = call("new_account")["account"]; priv = call("new_account", {"private": True})["account"]
if "--invisible" in sys.argv:
    # Start invisible: the private account claims USDC and WETH into itself (fee-exempt), then
    # borrows privately; the public account never holds anything.
    call("faucet_claim", {"claimer": priv, "to": priv, "token": usdc})
    call("faucet_claim", {"claimer": priv, "to": priv, "token": weth})
    e = call("faucet_claim", {"claimer": priv, "to": priv, "token": usdc}, ok=False); assert e["code"] == "faucet_cooldown", e
    m = next(x for x in call("markets") if x["collateral_asset"] == weth)["market"]
    b = call("balances", {"account": priv, "market": m})
    assert int(b["collateral"]) == 5 * 10**18 and int(b["loan"]) == 25_000 * 10**6, b
    assert native(pub) == 0
    op(op="supply_collateral", market=m, **{"from": priv}, amount=str(2 * 10**18))
    op(op="borrow", market=m, owner=priv, amount=str(1_000 * 10**6), to=priv)
    op(op="supply", market=m, **{"from": priv}, held=True, amount=str(5_000 * 10**6))
    call("sync")
    print("private:", call("balances", {"account": priv, "market": m}), "public native:", native(pub))
    lib.bos_app_close(app); print("OK"); sys.exit(0)
f = call("faucet", {"claimer": pub, "gas_claimer": priv})
assert native(pub) == 0
# A public claim with no native is refused before anything is sent.
e = call("faucet_claim", {"claimer": pub, "to": pub, "token": usdc}, ok=False); assert e["code"] == "payer_cannot_fund", e
# First gas: the private account claims (a private tx, fee-exempt), paid to the public one.
call("faucet_claim", {"claimer": priv, "to": pub})
gas = int(next(d["amount"] for d in f["drops"] if d["token"] is None))
assert native(pub) == gas, native(pub)
e = call("faucet_claim", {"claimer": priv, "to": pub}, ok=False); assert e["code"] == "faucet_cooldown", e
call("faucet_claim", {"claimer": pub, "to": pub, "token": usdc})
call("faucet_claim", {"claimer": pub, "to": pub, "token": weth})
call("faucet", {"claimer": pub, "gas_claimer": priv})
markets = call("markets")
m = next(x for x in markets if x["collateral_asset"] == weth)["market"]
b = call("balances", {"account": pub, "market": m})
assert int(b["collateral"]) == 5 * 10**18 and int(b["loan"]) == 25_000 * 10**6, b

op(op="supply_collateral", market=m, **{"from": pub}, amount=str(2 * 10**18))
op(op="borrow", market=m, owner=pub, amount=str(1_000 * 10**6))
print("native left after the public path:", native(pub))
if not public_only:
    call("transfer", {"from": pub, "to": priv, "token": weth, "amount": str(10**18)})
    call("sync")
    op(op="supply_collateral", market=m, **{"from": priv}, amount=str(10**18))
    op(op="borrow", market=m, owner=priv, amount=str(500 * 10**6), to=priv)
    call("sync")
    print("private USDC:", call("balances", {"account": priv, "market": m}))
call("positions", {"market": m, "owner": pub})
lib.bos_app_close(app); print("OK")
