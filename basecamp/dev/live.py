#!/usr/bin/env python3
"""Real-proof runs of the app API on the Basecamp test wallet (Basecamp closed: the app locks the
wallet), through `bos-app`, as the module would make them. Prints each answer, timing and job end.

  live.py STEP...   steps: state, sync, repay-part, repay-all, remove-all, takeback-all, unshield
"""
import json, os, subprocess, sys, threading, time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
INST = os.environ.get("BC_INSTANCE", "/tmp/bc-test/basecamp-user/module_data/bosphorus/4ad7c7110979")
E9 = dict(l.strip().split("=", 1) for l in open(f"{ROOT}/.localnet/e9.env") if "=" in l)
MARKET = E9["E9_MARKET"]


class App:
    def __init__(self):
        env = dict(os.environ, LEE_WALLET_HOME_DIR=f"{INST}/wallet")
        env.pop("RISC0_DEV_MODE", None)
        self.p = subprocess.Popen([f"{ROOT}/target/release/bos-app", "--lending", E9["E9_LENDING"], "--data-dir", f"{INST}/app"],
                                  stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=open("/tmp/bc-test/live.err", "a"), env=env, text=True, bufsize=1)
        self.n, self.ans, self.cv = 0, {}, threading.Condition()
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        for line in self.p.stdout:
            m = json.loads(line)
            if "event" not in m:
                with self.cv:
                    self.ans[m["id"]] = m
                    self.cv.notify_all()

    def call(self, method, params=None, timeout=900):
        self.n += 1
        i, t = self.n, time.time()
        self.p.stdin.write(json.dumps({"id": i, "method": method, "params": params or {}}) + "\n")
        self.p.stdin.flush()
        with self.cv:
            self.cv.wait_for(lambda: i in self.ans, timeout)
        return self.ans.get(i, {"error": {"code": "harness_timeout", "message": ""}}), time.time() - t

    def result(self, method, params=None):
        r, _ = self.call(method, params)
        if "error" in r:
            raise SystemExit(f"{method}: {r['error']}")
        return r["result"]

    def job(self, op):
        t = time.time()
        r, _ = self.call("submit", op)
        if "error" in r:
            print(f"  submit refused: {r['error']['code']}: {r['error']['message'][:200]}")
            return None
        jid = r["result"]["id"]
        while True:
            j = self.result("job", {"id": jid})
            if j["state"] in ("done", "failed", "interrupted", "unknown"):
                e = j.get("error") or {}
                print(f"  job {j['state']} in {time.time() - t:.1f} s, proving {(j.get('proving_ms') or 0) / 1000:.1f} s, block {j.get('block')}, {e.get('code')}: {str(e.get('message'))[:160]} note={j.get('note')}")
                return j
            time.sleep(1)

    def close(self):
        self.p.stdin.close()
        self.p.wait(timeout=900)


def accounts(app):
    acc = app.result("accounts")
    pub = [a["account"] for a in acc if a["account"].startswith("Public/")]
    priv = [a["account"] for a in acc if a["account"].startswith("Private/")]
    return acc, pub[0], priv[0]


def state(app):
    acc, pub, priv = accounts(app)
    info = app.result("info")
    print(f"  block {info['last_block']}, synced {info['synced_block']}")
    for a in acc:
        b = app.result("balances", {"account": a["account"], "market": MARKET})
        print(f"  {a['account'][:20]}… native {a['native']}, TKB {b['loan']}, TKA {b['collateral']}")
    for p in app.result("positions", {"market": MARKET, "owner": priv}):
        print(f"  position {'held' if p['held'] else 'slot ' + str(p['index'])}: saved {p.get('supplied_assets')}, security {p['collateral']}, debt {p.get('debt')}")


def main():
    app = App()
    _, pub, priv = accounts(app)
    for step in sys.argv[1:]:
        print(f"== {step}")
        if step == "state":
            state(app)
        elif step == "sync":
            r, dt = app.call("sync")
            print(f"  sync: {dt:.1f} s {r.get('result') or r.get('error')}")
        elif step == "repay-part":
            app.job({"op": "repay", "market": MARKET, "from": priv, "owner": priv, "index": 0, "amount": "2000000000", "unit": "assets"})
        elif step == "repay-all":
            app.job({"op": "repay", "market": MARKET, "from": priv, "owner": priv, "index": 0, "amount": "all"})
        elif step == "remove-all":
            app.job({"op": "withdraw_collateral", "market": MARKET, "owner": priv, "index": 0, "amount": "all"})
        elif step == "takeback-all":
            app.job({"op": "withdraw", "market": MARKET, "owner": priv, "held": True, "amount": "all"})
        elif step == "eat-supply":
            # The proxy drops the sequencer's reply to this tx's send (wallet pointed at :3041).
            def flip():
                while "sendTransaction [eat-send]" not in open("/tmp/bc-test/proxy.log").read()[-4000:]:
                    time.sleep(0.5)
                open("/tmp/bc-test/proxy.mode", "w").write("pass")
            open("/tmp/bc-test/proxy.mode", "w").write("eat-send")
            threading.Thread(target=flip, daemon=True).start()
            app.job({"op": "supply", "market": MARKET, "from": priv, "held": True, "amount": "1000000000"})
            open("/tmp/bc-test/proxy.mode", "w").write("pass")
        elif step == "kill-supply":
            r, _ = app.call("submit", {"op": "supply", "market": MARKET, "from": priv, "held": True, "amount": "1000000000"})
            jid = r["result"]["id"]
            while app.result("job", {"id": jid})["state"] != "proving":
                time.sleep(0.5)
            time.sleep(20)
            app.p.kill(); app.p.wait()
            t = time.time()
            app = App()
            j = app.result("job", {"id": jid})
            while j["state"] in ("proving", "checking"):
                time.sleep(0.5); j = app.result("job", {"id": jid})
            print(f"  killed 20 s into the proof; {time.time() - t:.1f} s after restart: {j['state']} tx={j.get('tx')} note={j.get('note')}")
        elif step == "unshield":
            r, dt = app.call("transfer", {"from": priv, "to": pub, "amount": "1000000"})
            print(f"  unshield 1,000,000 LEZ: {dt:.1f} s {r.get('result') or r.get('error')}")
    app.close()


if __name__ == "__main__":
    main()
