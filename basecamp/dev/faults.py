#!/usr/bin/env python3
"""Failure scenarios for the app API, against the dev-mode localnet through faultproxy.py.

  faults.py SCENARIO...   (or `all`)

Copies the p6 wallet to /tmp/fault/wallet with its sequencer pointed at the proxy (127.0.0.1:3041),
runs `bos-app` on it (RISC0_DEV_MODE=1), sets the proxy's mode and reports what the app answered,
how long it took, and how each job ended. Needs the proxy running (`faultproxy.py &`), the p6
localnet, `.localnet/{deploy,e9}.env`.
"""
import json
import os
import shutil
import subprocess
import sys
import threading
import time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
MODE_FILE = "/tmp/bc-test/proxy.mode"
WORK = "/tmp/fault"


def env_file(path):
    out = {}
    for line in open(path):
        if "=" in line:
            k, v = line.strip().split("=", 1)
            out[k] = v
    return out


DEPLOY = env_file(f"{ROOT}/.localnet/deploy.env")
E9 = env_file(f"{ROOT}/.localnet/e9.env")
LENDING, MARKET, PAYER = E9["E9_LENDING"], E9["E9_MARKET"], DEPLOY["PAYER"]


def set_mode(m):
    with open(MODE_FILE, "w") as f:
        f.write(m)


class App:
    def __init__(self, fresh=True, extra_env=None):
        # One copy of the p6 wallet, kept across runs: a fresh copy would derive the same "new"
        # private accounts again, whose notes are already on the chain (the sequencer then drops
        # their txs: "Commitment already seen"). `fresh` only clears the jobs.
        if fresh:
            shutil.rmtree(f"{WORK}/data", ignore_errors=True)
        burn = not os.path.exists(f"{WORK}/wallet/storage.json")
        if burn:
            shutil.copytree(f"{ROOT}/.localnet/p6-wallet", f"{WORK}/wallet", ignore=shutil.ignore_patterns("*.lock"))
            cfg_path = f"{WORK}/wallet/wallet_config.json"
            cfg = json.load(open(cfg_path))
            cfg["sequencer"]["sequencer_addr"] = "http://127.0.0.1:3041"
            json.dump(cfg, open(cfg_path, "w"), indent=2)
        env = dict(os.environ, RISC0_DEV_MODE="1", LEE_WALLET_HOME_DIR=f"{WORK}/wallet", **(extra_env or {}))
        self.err = open(f"{WORK}/app.err", "a")
        self.p = subprocess.Popen(
            [f"{ROOT}/target/release/bos-app", "--lending", LENDING, "--data-dir", f"{WORK}/data"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.err, env=env, text=True, bufsize=1,
        )
        self.next_id, self.answers, self.events = 0, {}, []
        self.cv = threading.Condition()
        threading.Thread(target=self._read, daemon=True).start()
        if fresh and burn:
            # Skip the private keys earlier copies may have used.
            for _ in range(40):
                self.call("new_account", {"private": True})

    def _read(self):
        for line in self.p.stdout:
            msg = json.loads(line)
            with self.cv:
                if "event" in msg:
                    self.events.append(msg["job"])
                else:
                    self.answers[msg["id"]] = msg
                self.cv.notify_all()

    def call(self, method, params=None, timeout=300):
        self.next_id += 1
        i = self.next_id
        t0 = time.time()
        self.p.stdin.write(json.dumps({"id": i, "method": method, "params": params or {}}) + "\n")
        self.p.stdin.flush()
        with self.cv:
            self.cv.wait_for(lambda: i in self.answers, timeout)
        dt = time.time() - t0
        r = self.answers.get(i, {"error": {"code": "harness_timeout", "message": f"no answer in {timeout} s"}})
        return r, dt

    def wait_job(self, job_id, timeout=600, until=None):
        until = until or (lambda j: j["state"] in ("done", "failed", "interrupted", "unknown"))
        deadline = time.time() + timeout
        while time.time() < deadline:
            r, _ = self.call("job", {"id": job_id})
            j = r.get("result")
            if j and until(j):
                return j
            time.sleep(1)
        return self.call("job", {"id": job_id})[0].get("result")

    def kill(self):
        self.p.kill()
        self.p.wait()

    def close(self):
        self.p.stdin.close()
        self.p.wait(timeout=600)


def show(label, r, dt):
    if "error" in r:
        print(f"  {label}: {dt:5.1f} s  ERROR {r['error']['code']}: {r['error']['message'][:200]}")
    else:
        s = json.dumps(r["result"])
        print(f"  {label}: {dt:5.1f} s  ok {s[:160]}")


def job_line(j):
    e = j.get("error") or {}
    return f"state={j['state']} tx={(j.get('tx') or '-')[:12]} block={j.get('block')} error={e.get('code')}: {str(e.get('message'))[:160]} note={str(j.get('note'))[:160]}"


def position_supply(app, owner):
    r, _ = app.call("positions", {"market": MARKET, "owner": owner})
    ps = r.get("result") or []
    return [p["supply_shares"] for p in ps]


# A public account of the p6 wallet holding TKA/TKB (minted to it) and some native LEZ.
FUNDED = "Public/8Rzock62A1CePkgpKw7YprWhuM6qeLUuEwm2NQe7ByT"
SUPPLY = {"op": "supply", "market": MARKET, "from": FUNDED, "amount": "1000000"}


def ensure_native(app):
    r, _ = app.call("accounts")
    native = {a["account"]: int(a["native"] or 0) for a in r["result"]}
    if native.get(FUNDED, 0) < 10**8:
        show("fund native", *app.call("transfer", {"from": PAYER, "to": FUNDED, "amount": str(10**9)}))


def new_private(app, tkb="5000000"):
    priv = app.call("new_account", {"private": True})[0]["result"]["account"]
    show("token shield", *app.call("transfer", {"from": FUNDED, "to": priv, "amount": tkb, "token": DEPLOY["TKB"]}))
    return priv


def s_down():
    print("== down: the sequencer refuses connections")
    app = App()
    set_mode("pass")
    ensure_native(app)
    set_mode("down")
    for m, p in [("info", {}), ("accounts", {}), ("markets", {}), ("market", {"market": MARKET}), ("quote", SUPPLY), ("sync", {})]:
        show(m, *app.call(m, p))
    r, dt = app.call("submit", SUPPLY)
    show("submit", r, dt)
    j = app.wait_job(r["result"]["id"], 120)
    print("  job:", job_line(j))
    show("transfer", *app.call("transfer", {"from": FUNDED, "to": PAYER, "amount": "1"}))
    set_mode("pass")
    app.close()


def s_hang():
    print("== hang: the sequencer accepts connections and never answers")
    app = App()
    set_mode("hang")
    for m, p in [("info", {}), ("quote", SUPPLY)]:
        show(m, *app.call(m, p, timeout=400))
    set_mode("pass")
    app.close()


def s_eat_send():
    print("== eat-send: a public supply reaches the sequencer, its reply is lost")
    app = App()
    set_mode("pass")
    ensure_native(app)
    before = position_supply(app, FUNDED)
    set_mode("eat-send")
    r, dt = app.call("submit", dict(SUPPLY, payer=PAYER))
    jid = r["result"]["id"]
    time.sleep(3)
    set_mode("pass")
    j = app.wait_job(jid, 300)
    print("  job:", job_line(j))
    print(f"  supply shares before {before}, after {position_supply(app, FUNDED)}")
    app.close()


def s_eat_send_private():
    print("== eat-send, private: a held supply reaches the sequencer, its reply is lost")
    app = App()
    set_mode("pass")
    ensure_native(app)
    priv = new_private(app)
    set_mode("eat-send")
    r, dt = app.call("submit", {"op": "supply", "market": MARKET, "from": priv, "held": True, "amount": "1000000"})
    jid = r["result"]["id"]
    time.sleep(3)
    set_mode("pass")
    j = app.wait_job(jid, 300)
    print("  job:", job_line(j))
    print("  held position:", [(p["held"], p["supply_shares"]) for p in app.call("positions", {"market": MARKET, "owner": priv})[0].get("result", [])])
    app.close()


def s_transfer_eat():
    print("== eat-send, transfer: a shield reaches the sequencer, its reply is lost")
    app = App()
    set_mode("pass")
    ensure_native(app)
    priv = app.call("new_account", {"private": True})[0]["result"]["account"]
    set_mode("eat-send")
    t = threading.Timer(3, lambda: set_mode("pass"))
    t.start()
    show("shield 1000 native", *app.call("transfer", {"from": FUNDED, "to": priv, "amount": "1000"}))
    show("sync", *app.call("sync"))
    r, _ = app.call("accounts")
    print("  private balance after sync:", [a["native"] for a in r["result"] if a["account"] == priv])
    app.close()


def s_short():
    print("== more than the account holds")
    app = App()
    set_mode("pass")
    priv = app.call("new_account", {"private": True})[0]["result"]["account"]
    show("shield 10^30 TKB", *app.call("transfer", {"from": FUNDED, "to": priv, "amount": str(10**30), "token": DEPLOY["TKB"]}))
    show("quote supply 10^30", *app.call("quote", dict(SUPPLY, amount=str(10**30))))
    show("quote add security 10^30", *app.call("quote", {"op": "supply_collateral", "market": MARKET, "from": FUNDED, "amount": str(10**30)}))
    app.close()


def s_reject_send():
    print("== reject-send: sendTransaction answered 502, nothing forwarded")
    app = App()
    set_mode("pass")
    ensure_native(app)
    set_mode("reject-send")
    r, dt = app.call("submit", dict(SUPPLY, payer=PAYER))
    jid = r["result"]["id"]
    j = app.wait_job(jid, 200, until=lambda j: j["state"] not in ("queued", "preparing", "sending"))
    set_mode("pass")
    print("  job after the 502:", job_line(j))
    app.close()


def s_reads_during_proof():
    print("== reads while a job proves (stretched to 30 s) and a sync waits behind it")
    app = App()
    set_mode("pass")
    ensure_native(app)
    priv = new_private(app)
    app.close()
    app = App(fresh=False, extra_env={"BOS_APP_DELAY_SEND_MS": "30000"})
    r, _ = app.call("submit", {"op": "supply", "market": MARKET, "from": priv, "held": True, "amount": "1000000"})
    jid = r["result"]["id"]
    app.wait_job(jid, 60, until=lambda j: j["state"] == "proving")
    res = {}
    th = threading.Thread(target=lambda: res.update(sync=app.call("sync")))
    th.start()
    time.sleep(1)
    for m, p in [("info", {}), ("accounts", {}), ("markets", {}), ("positions", {"market": MARKET, "owner": priv})]:
        show(m + " (during the proof)", *app.call(m, p))
    th.join()
    show("sync (queued behind the proof)", *res["sync"])
    print("  job:", job_line(app.wait_job(jid, 120)))
    app.close()


def s_kill_proving():
    print("== killed while proving (a held supply, proof stretched to 30 s): what the restart does")
    app = App()
    set_mode("pass")
    ensure_native(app)
    priv = new_private(app)
    app.close()
    app = App(fresh=False, extra_env={"BOS_APP_DELAY_SEND_MS": "30000"})
    r, _ = app.call("submit", {"op": "supply", "market": MARKET, "from": priv, "held": True, "amount": "1000000"})
    jid = r["result"]["id"]
    app.wait_job(jid, 60, until=lambda j: j["state"] == "proving")
    app.kill()
    t0 = time.time()
    app = App(fresh=False)
    j = app.wait_job(jid, 120, until=lambda j: j["state"] != "proving")
    print(f"  after restart ({time.time() - t0:.0f} s):", job_line(j))
    app.close()


SCENARIOS = {"down": s_down, "hang": s_hang, "eat-send": s_eat_send, "eat-send-private": s_eat_send_private, "transfer-eat": s_transfer_eat,
             "short": s_short, "reject-send": s_reject_send, "reads-during-proof": s_reads_during_proof, "kill-proving": s_kill_proving}

if __name__ == "__main__":
    names = sys.argv[1:] or ["all"]
    for n in (list(SCENARIOS) if names == ["all"] else names):
        SCENARIOS[n]()
        sys.stdout.flush()
    set_mode("pass")
