# Bosphorus

A lending market on [Logos LEZ](https://github.com/logos-blockchain/logos-execution-zone) v0.3.1:
Morpho Blue-style isolated, immutable markets with some Aave v3 risk features, plus privacy.
Positions are public, so anyone can check health and liquidate. Their owners can be hidden: a
private account acts on its positions through private transactions, and nothing published links
a position back to its owner.

Status: experimental. Runs on a local dev-mode sequencer and on the LEZ testnet (see
[DEPLOYMENTS.md](DEPLOYMENTS.md)). Not audited.

## What's here

| Path | Crate | What it does |
|---|---|---|
| `core/` | `bosphorus-core` | All the maths (shares, interest, IRM, health, liquidation, claim intervals). Pure Rust, no LEZ types |
| `programs/token/` | `bosphorus-token` | Fungible token for v0.3.1, shaped like the native token; one program instance per asset |
| `programs/oracle/` | `bosphorus-oracle` | Push price feeds, U256 prices at Morpho's 1e36 scale |
| `programs/lending/` | `bosphorus-lending` | Markets, positions, borrowing, liquidation, fees, asset allowlist |
| `programs/methods/` | `bosphorus-methods` | Builds the guests (risc0) and holds the in-process tests |
| `client/` | | Builds, checks and sends txs on top of LEZ's `wallet` crate; block scanner for markets and positions |
| `cli/` | `bos` | Command line for tokens, oracle, lending admin, markets and positions |
| `keeper/` | `bos-keeper` | Liquidation bot: finds positions by scanning blocks, liquidates unhealthy ones in trusted markets |
| `app/` | `bos-app` | JSON-lines API over stdin/stdout for front-ends, with jobs that survive restarts |
| `spikes/` | | Platform experiments from the start of the project (gas, proving, privacy) |
| `scripts/` | | End-to-end scripts per phase, testnet deploy and scenarios, price pusher |

## How it works

LEZ programs can't read state while planning a transaction, and each `apply` sees only one account
shard. So every op carries claims about state it can't see (a ratio interval, a price interval, a
validity window), and each shard checks the claims that concern it. The maths in `core` makes sure
any value inside a claimed interval leaves the protocol no worse off.

Private-mode ops are proven locally (around 10 minutes on an M1), so their claims are sized for
that delay. Position addresses come from an HMAC of the private account's secret key, so a
private owner can recover their positions from the mnemonic alone, and nobody else can link
them. Lenders can also hold supply shares on their own private account (`--held`), with no public
position at all.

## Build

Needs the Rust toolchain in `rust-toolchain.toml` and the risc0 toolchain (`rzup`) for the guests.
Guests are built locally with `risc0_build::embed_methods`, not Docker, so image ids differ from
Docker builds.

```sh
cargo build --release -p bosphorus-cli -p bosphorus-keeper -p bosphorus-app
cargo test -p bosphorus-core
RISC0_DEV_MODE=1 cargo test -p bosphorus-methods
```

## Run locally

You need a LEZ checkout at tag `v0.3.1` for the sequencer and the wallet:

```sh
cd ../logos-execution-zone   # at v0.3.1
cargo build --release --features standalone -p sequencer_service
cargo build --release -p wallet
```

Start a dev-mode sequencer from this repo (state goes in `.localnet/`, delete it to reset):

```sh
mkdir -p .localnet
RISC0_DEV_MODE=1 ../logos-execution-zone/target/release/sequencer_service \
  scripts/localnet/sequencer_config.json > .localnet/sequencer.log 2>&1 &
```

Then run the end-to-end scripts. They expect the wallet binary at
`~/Work/logos-execution-zone/target/release/wallet`; set `WALLET=` to point elsewhere.

```sh
scripts/phase2-e2e.sh    # tokens, oracle, a market, the public lending lifecycle
scripts/phase3-e2e.sh    # liquidations by the keeper, fee claims
scripts/phase4-e2e.sh    # private positions, recovery by index, tx privacy reports
scripts/e9-held.sh       # lender shares held on a private account
scripts/app-e2e.sh       # the bos-app API, including restarts mid-job
```

`NETWORK=testnet` points the Phase 5 scripts (`phase5-deploy.sh`, `phase5-public.sh`,
`phase5-private.sh`) at `https://testnet.lez.logos.co` with real proofs.

## Using `bos`

```sh
bos check --lending <id>                     # privacy circuit and deployed programs match this build
bos market list --lending <id>
bos status --lending <id> --market <hex>
bos market supply --lending <id> --market <hex> --from Public/... --assets 1000000
bos market borrow --lending <id> --market <hex> --owner Private/... --index 0 --assets 500000
bos tx-report --tx <hash> --owner Private/... --market <hex> --check  # what a tx published, scanned for the owner's bytes
```

Any op with a `Private/` owner, sender or payer goes out as a private transaction. `bos --help`
lists the rest.

## Caveats

- Targets LEZ v0.3.1 only. Earlier versions are incompatible.
- Programs are immutable, so a new build means a new deployment. `bos`, the keeper and the app
  refuse programs that aren't their own build (image ids). To prove a private tx against a
  deployment, you need the exact ELFs it was built from (`bos programs --write-dir`).
- The lending guest changed after the testnet deployment in `DEPLOYMENTS.md`, so that deployment
  runs older code than `main`.

## License

MIT OR Apache-2.0.
