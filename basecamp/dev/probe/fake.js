.pragma library
// App-API answers in the shapes app/src/lib.rs returns, for the view probe (localnet-like data).
var TKB = "Public/A6DaCUx7ii5bJwmfMGqpVSQQHRk6b9WTYVrbtbQ1PWiT"
var TKA = "Public/9xg1XTEuSwKK28AEUDce91eC1DHFoJndw7W8q7EunMAK"
var MARKET = "efebea8f489917d403bbf9ba96b82b67246d889e378c4fbd559c69d4fbfbc941"
var USDC = "Public/Cz65QuneQ6rV65Czia6uJadq1ASJmGiZUPDVW5ZJPgC3"
var WBTC = "Public/CmSm9MJZ48FKZctCYTADjPKpgepvz8P5du8uqGZzaqpK"
var MARKET2 = "9d09475566f9b61ccb74e0996b32da2af9116bbe9e673c56693efb3ad367d246"
var PUB = "Public/MNndQX7ZXHnKYQVCairKEXa1FjUDSjE4fXUX7rc2LhU"
var PRIV = "Private/7Bd6DDQUhtZsd88A85KHJ1Cpsw7ALt1CCsiGR6u3gd1F"
var now = 1793023400000

var market = { market: MARKET, loan_asset: TKB, collateral_asset: TKA, oracle_program: "Public/HD9S2yYXAzAnpFBBEWQ8rdCqSABa6QGYTxuV1JupCsEz",
    oracle_feed: "Public/Btfccbco4YUvrXu6FNuE8UkrgRZS4FVWaWk9EquUxjsY", lltv: "0.800000000000000000", max_price_age_ms: 3600000,
    supplied: "1071001355711", borrowed: "70001214778", utilization: "0.065359477124183006", borrow_apr: "0.028232553800604800",
    supply_apr: "0.001845264886575710", fee: "0.000000000000000000", loan_decimals: 6, collateral_decimals: 6 }

var market2 = JSON.parse(JSON.stringify(market))
market2.market = MARKET2; market2.loan_asset = USDC; market2.collateral_asset = WBTC; market2.collateral_decimals = 8
market2.oracle_feed = "Public/4RgSnfvzxcQo1Dpp3zQsJ62BofthsSJq9NNU7k6bEhrr"; market2.supplied = "1000000000000"; market2.borrowed = "0"; market2.utilization = "0"

// An error answer (the probe's core wraps it as {"error"}).
function fail(code, message) { return { __error: { code: code, message: message } } }

function answer(method, p, opts) {
    if (opts.offline && method === "markets") return [market, market2].map(function(m) { var c = JSON.parse(JSON.stringify(m)); c.cached = true; return c })
    if (opts.offline && ["market", "positions", "balances", "quote", "sync", "transfer", "submit", "pool", "pool_deposit", "pool_withdraw", "pool_ragequit"].indexOf(method) >= 0)
        return fail("network", "client error (SendRequest): connection error: Connection refused (os error 61)")
    if (opts.offline && method === "info")
        return { lending: "Public/DpQfghec2FLsR813PCJbbc6wuLajnNNGwDsXM4poARKS", sequencer: "http://127.0.0.1:3040", last_block: null, synced_block: 1014,
                 network_problem: "client error (SendRequest): connection error: Connection refused (os error 61)", privacy_circuit_problem: null, lending_problem: null }
    switch (method) {
    case "info": return { lending: "Public/DpQfghec2FLsR813PCJbbc6wuLajnNNGwDsXM4poARKS", sequencer: "http://127.0.0.1:3040", last_block: 1014, synced_block: 1014,
                          network_problem: null, syncing: null, privacy_circuit_problem: null, lending_problem: null }
    case "accounts": return [{ account: PUB, labels: [], native: "899218432" }, { account: PRIV, labels: [], native: "100000000" }]
    case "markets": return [market, market2]
    case "market": {
        var d = JSON.parse(JSON.stringify(market))
        d.accounts = { market: "Public/74iyw68Dn1H7Gk7SwZdchF9fCucRxzso7xrvrTtsBm3m", loan_vault: "Public/AnNNkNPj4PYH4zAEvtSFTnSWJmWuLLtVNpwA1nsjab1a", collateral_vault: "Public/5hwZd7PUgpV8vKexPuDJmqYrxVf1FsUDUn6UPCJFfKAQ" }
        d.feed = { price: "2500000000000000000000000000000000000", updated_at: now - (opts.stale ? 18528446 : 420000), age_ms: opts.stale ? 18528446 : 420000, round: 6 }
        d.code_problem = null
        return d
    }
    case "positions":
        if (p.owner === PUB) return [{ index: null, held: false, position: "Public/9BR8QsSmYa3o3tcD41PAVkYv28FH2C9YBGys6drfEB7R", supply_shares: "9999988011104373",
                                       collateral: "0", borrow_shares: "0", supplied_assets: "9999999996", debt: "0", max_borrow: "0", health_factor: null }]
        return [{ index: 0, held: false, position: "Public/4kJ…", supply_shares: "0", collateral: "50000000000", borrow_shares: "61000000000000000",
                  supplied_assets: "0", debt: "61730114210", max_borrow: "100000000000", health_factor: "1.620000000000000000" },
                { index: null, held: true, position: PRIV, supply_shares: "9999985226281826", collateral: "0", borrow_shares: "0",
                  supplied_assets: "9999999990", debt: "0", max_borrow: "0", health_factor: null }]
    case "balances":
        if (p.market === MARKET2)
            return p.account === PUB ? { account: PUB, loan_asset: USDC, loan: "180000000000", collateral_asset: WBTC, collateral: "190000000" }
                                     : { account: PRIV, loan_asset: USDC, loan: "0", collateral_asset: WBTC, collateral: "200000000" }
        return p.account === PUB ? { account: PUB, loan_asset: TKB, loan: "90000000000", collateral_asset: TKA, collateral: "0" }
                                 : { account: PRIV, loan_asset: TKB, loan: "10000000000", collateral_asset: TKA, collateral: "25000000000" }
    case "quote": return { op: p.op, private: String(p.owner || p.from).indexOf("Private/") === 0, position: p.held ? p.owner : "Public/9BR8…", held: !!p.held,
                           field: "supply_shares", from: "0", to: "9999985226281826", amount: "9999985226281826", valid_until: now + 1200000, payer: null,
                           health_factor_after: p.op === "borrow" ? "1.310000000000000000" : null }
    case "submit": return { id: "1793023500000-0000", state: "queued" }
    case "jobs": return [
        { id: "1793021260321-0000", request: { op: "supply", market: MARKET, from: PUB, owner: PUB, amount: "10000000000" }, state: "done", created_ms: now - 2200000,
          since_ms: now - 2190000, expect: { private: [] }, tx: "fb6a60b7306f8fe3e8fff31410d28470c5da30ee0b26b1fccc0ddbbccc4db252", block: 883, proving_ms: null, error: null, note: null },
        { id: "1793023149328-0000", request: { op: "supply", market: MARKET, from: PRIV, owner: PRIV, held: true, amount: "10000000000" }, state: "done", created_ms: now - 250000,
          since_ms: now - 170000, expect: { private: [PRIV] }, tx: "0bb1274dfff716b40a5c7277f99633a5da949af2bc639516e663a05e36140b09", block: 1014, proving_ms: 71606, error: null, note: null },
        { id: "1793023300000-0000", request: { op: "borrow", market: MARKET, owner: PRIV, index: 0, amount: "5000000000" }, state: "failed", created_ms: now - 100000,
          since_ms: now - 95000, expect: null, tx: null, block: null, proving_ms: null, error: { code: "stale_price", message: "feed too old" }, note: null },
        { id: "1793023400000-0000", request: { op: "repay", market: MARKET, from: PRIV, owner: PRIV, index: 0, amount: "1000000000", unit: "assets" }, state: "proving", created_ms: now - 30000,
          since_ms: Date.now() - 25000, expect: { private: [PRIV] }, tx: null, block: null, proving_ms: null, error: null, note: null },
        { id: "1793023410000-0000", request: { op: "borrow", market: MARKET, owner: PRIV, index: 0, amount: "2000000000" }, state: "interrupted", created_ms: now - 20000,
          since_ms: now - 5000, expect: { private: [PRIV] }, tx: null, block: null, proving_ms: null, error: null, note: "Bosphorus stopped during the proof, before anything was sent" },
        { id: "1793023420000-0000", request: { op: "supply", market: MARKET, from: PUB, owner: PUB, amount: "3000000000" }, state: "submitted", created_ms: now - 10000,
          since_ms: now - 9000, expect: { private: [] }, tx: "8ebf876ab943aa", block: null, proving_ms: null, error: null,
          note: "not on the chain 8 blocks after sending, so most likely dropped; it can't land once its offer ends, and Bosphorus watches until then" },
        { id: "1793023430000-0000", request: { op: "supply", market: MARKET, from: PUB, owner: PUB, amount: "4000000000" }, state: "failed", created_ms: now - 5000,
          since_ms: now - 4000, expect: { private: [] }, tx: "1234abcd", block: null, proving_ms: null, note: null,
          error: { code: "rejected", message: "the network refused the transaction: Invalid nonce" } }
    ]
    case "transfer": return { tx: "840bf35862b2232694597e9e191eb12449431b8a7e33f7f6b9d0c7e945bc6382" }
    case "pool": return { pool: "Public/HyWkqC3kJ4N6ewpYoHgvBmKpfp9oWRrZBj25dg4G6HCb", asp: PUB, approved: 3, lots: [
        { account: PRIV, label: "57299104799b5f92818acc37a031d25fcf44030114ced615e7402327a4061bfe", token: null, amount: "20000000", state: "approved", depositor: PUB, can_take_back: true },
        { account: PRIV, label: "ddfede486de510769d33ffc092f60f7346ac2abbcb606bfc2778528ec415ff34", token: TKB, amount: "2000000000", state: "approved", depositor: PUB, can_take_back: true },
        { account: PRIV, label: "a1fede486de510769d33ffc092f60f7346ac2abbcb606bfc2778528ec415ff34", token: TKB, amount: "1000000000", state: "approved", depositor: PUB, can_take_back: true },
        { account: PRIV, label: "c508209ed9b4c17473a681721a2cc4a4a17cce4b38a02faac83ba8d70c8f5f2a", token: TKB, amount: "500000000", state: "pending", depositor: PUB, can_take_back: true }] }
    case "pool_deposit": return { tx: "840bf35862b2232694597e9e191eb12449431b8a7e33f7f6b9d0c7e945bc6382", label: "57299104799b5f92818acc37a031d25fcf44030114ced615e7402327a4061bfe" }
    case "pool_withdraw":
    case "pool_ragequit": return { tx: "840bf35862b2232694597e9e191eb12449431b8a7e33f7f6b9d0c7e945bc6382" }
    case "sync": return { synced_block: 1014 }
    case "new_account": return { account: "Public/NEW1111111111111111111111111111111111111" }
    case "open": return { prover: { gpu: true, keccak_po2: 14 } }
    case "create_wallet": return { mnemonic: "humor sick retreat love culture benefit width animal cement trade crash brave zebra region enough dolphin idea seed bronze better cram chimney east muffin" }
    }
    return null
}

function settings() {
    var t = {}; t[TKB] = "TKB"; t[TKA] = "TKA"; t[USDC] = "USDC"; t[WBTC] = "WBTC"
    return { sequencer: "http://127.0.0.1:3040", lending: "Public/DpQfghec2FLsR813PCJbbc6wuLajnNNGwDsXM4poARKS", tokens: t }
}
