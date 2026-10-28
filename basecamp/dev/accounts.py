import ctypes, json, os, sys
lib = ctypes.CDLL(os.path.expanduser("~/Work/money-market/target/release/libbosphorus_ffi.dylib"))
class Open(ctypes.Structure): _fields_=[("app",ctypes.c_void_p),("result_json",ctypes.c_void_p)]
lib.bos_app_open.restype = Open; lib.bos_app_open.argtypes=[ctypes.c_char_p]
lib.bos_app_call.restype = ctypes.c_void_p; lib.bos_app_call.argtypes=[ctypes.c_void_p, ctypes.c_char_p]
lib.bos_app_close.argtypes=[ctypes.c_void_p]
o = lib.bos_app_open(json.dumps({"wallet_home": "/tmp/bc-test/wallet", "lending": sys.argv[1]}).encode())
r = json.loads(ctypes.string_at(lib.bos_app_call(o.app, b'{"id":1,"method":"accounts"}')))
for a in r["result"]: print(a["account"], a["native"])
lib.bos_app_close(o.app)
