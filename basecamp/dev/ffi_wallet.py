import ctypes, json, os, subprocess, sys, time, shutil
lib = ctypes.CDLL(os.path.expanduser("~/Work/money-market/target/release/libbosphorus_ffi.dylib"))
lib.bos_wallet_create.restype = ctypes.c_void_p; lib.bos_wallet_create.argtypes=[ctypes.c_char_p]
class Open(ctypes.Structure): _fields_=[("app",ctypes.c_void_p),("result_json",ctypes.c_void_p)]
lib.bos_app_open.restype = Open; lib.bos_app_open.argtypes=[ctypes.c_char_p]
lib.bos_app_call.restype = ctypes.c_void_p; lib.bos_app_call.argtypes=[ctypes.c_void_p, ctypes.c_char_p]
lib.bos_app_string_free.argtypes=[ctypes.c_void_p]; lib.bos_app_close.argtypes=[ctypes.c_void_p]
def take(p):
    s = ctypes.string_at(p).decode(); lib.bos_app_string_free(p); return json.loads(s)
home = "/tmp/bc-test/wallet"; shutil.rmtree(home, ignore_errors=True)
r = take(lib.bos_wallet_create(json.dumps({"wallet_home": home, "sequencer": "http://127.0.0.1:3040"}).encode()))
print("create:", "mnemonic words:", len(r["result"]["mnemonic"].split()) if "result" in r else r)
again = take(lib.bos_wallet_create(json.dumps({"wallet_home": home, "sequencer": "http://127.0.0.1:3040"}).encode()))
print("create again:", again.get("error"))
lending = sys.argv[1]
o = lib.bos_app_open(json.dumps({"wallet_home": home, "lending": lending}).encode()); print("open:", take(o.result_json)); app = o.app
n = [0]
def call(method, params={}):
    n[0]+=1; t=time.time(); r = take(lib.bos_app_call(app, json.dumps({"id": n[0], "method": method, "params": params}).encode()))
    print(f"{method} ({time.time()-t:.1f} s):", json.dumps(r.get("result", r.get("error")))[:300]); return r.get("result")
pub = call("new_account")["account"]; priv = call("new_account", {"private": True})["account"]
print("FUND", pub, flush=True)
# wait for funding by the outer script
while not os.path.exists("/tmp/bc-test/funded"): time.sleep(2)
call("accounts")
call("transfer", {"from": pub, "to": priv, "amount": "1000000"})
call("transfer", {"from": pub, "to": "Public/CbgR6tj5kWx5oziiFptM7jMvrQeYY3Mzaao6ciuhSr2r", "amount": "1000"})
call("sync"); call("accounts"); call("info")
call("transfer", {"from": priv, "to": "Private/CbgR6tj5kWx5oziiFptM7jMvrQeYY3Mzaao6ciuhSr2r", "amount": "1"})
lib.bos_app_close(app); print("closed")
