import ctypes, json, os, sys, time
lib = ctypes.CDLL(os.path.expanduser("~/Work/money-market/target/release/libbosphorus_ffi.dylib"))
class Open(ctypes.Structure): _fields_=[("app",ctypes.c_void_p),("result_json",ctypes.c_void_p)]
lib.bos_app_open.restype = Open; lib.bos_app_open.argtypes=[ctypes.c_char_p]
lib.bos_app_call.restype = ctypes.c_void_p; lib.bos_app_call.argtypes=[ctypes.c_void_p, ctypes.c_char_p]
lib.bos_app_close.argtypes=[ctypes.c_void_p]
t=time.time(); o = lib.bos_app_open(json.dumps({"wallet_home": "/tmp/bc-test/wallet", "lending": sys.argv[1]}).encode()); print(f"open {time.time()-t:.1f} s", flush=True)
time.sleep(int(sys.argv[2]))
t=time.time()
r = json.loads(ctypes.string_at(lib.bos_app_call(o.app, json.dumps({"id":1,"method":"transfer","params":{"from":"Public/3FbfnavYYsdZwfuJNnQ9BU67ULqCmvp4aU5fFS3QSkMT","to":"Private/7491Zr3obFsnTGLJYCL1qKNPgte9koLKFAgFnGz3Ts9v","amount":"5"}}).encode())))
print(f"shield {time.time()-t:.1f} s", r, flush=True)
lib.bos_app_close(o.app)
