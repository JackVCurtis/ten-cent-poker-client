# ten-cent-poker-client

Desktop client for **Ten-Cent Poker** — a fully peer-to-peer, zero-knowledge poker game. It is a
self-contained Rust application that bundles an EVM wallet, a ZKP-based decentralized card-dealing
engine (mental poker), peer-to-peer networking, and a desktop GUI. There are no servers: you find a
table by sharing a **table URI** over email or chat.

The on-chain side (buy-in / ZKP-checked settlement) lives in the sibling
[ten-cent-poker-contract](../ten-cent-poker-contract) repo.

## Verification-first architecture

The synchronous game logic is **formally verified with [Verus](https://verus-lang.github.io/verus/)**;
asynchronous code (networking, RPC, GUI) is intentionally out of scope, per design. The workspace is
split so the two never entangle:

| Crate | Verified? | Role |
|-------|-----------|------|
| `core/`   | **Verus** | pure synchronous logic: pot/chip math, winner selection, game-state invariants |
| `wallet/` | no (tested) | BIP-39/32 HD wallet + signing on `alloy` |
| `crypto/` | no (tested) | `arkworks` BN254 — mental-poker shuffle + Groth16 settlement proofs |
| `net/`    | no (tested) | `libp2p` peer-to-peer transport + table-URI discovery |
| `app/`    | no | `eframe`/`egui` desktop binary tying it together |

`core/` is deliberately **not** a cargo workspace member: it is checked with `./tools/verus`, not
`cargo build`, which keeps the async crates free of any `vstd` coupling. The async crates are an
ordinary cargo workspace (`wallet`, `crypto`, `net`, `app`).

## Build & run (cargo)

```bash
cargo build                      # builds wallet + crypto + net + app
cargo test                       # unit tests across the crates
cargo run -p poker-app           # headless smoke: prints a wallet address, a table URI, BN254 check
cargo run -p poker-app --features gui -- --gui   # launch the egui desktop window (needs a display)
```

> On the 9p shared mount in the dev VM, build to local disk to avoid proc-macro `.so`
> dlopen segfaults: `export CARGO_TARGET_DIR=$HOME/poker-client-target`. Not needed on a Mac host.

The headless smoke output looks like:

```
wallet : account 0 address = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266
net    : table URI         = tencentpoker:/ip4/127.0.0.1/udp/9000/quic-v1/p2p/12D3Koo…
crypto : BN254 scalar field = 254 bits (g·42 computed)
```

## Formal verification (Verus)

```bash
./verify.sh
# => verification results:: 3 verified, 0 errors
```

The verified core currently proves:
- `amount_to_call` — chips conserved when matching the current bet.
- `winner_by_stack` — returns an in-bounds index that is a true maximum (loop-invariant proof).

Per the `hello_verus` convention this has a *negative test*: deleting the load-bearing
`forall` loop invariant in `winner_by_stack` makes Verus report `2 verified, 1 errors`
("failed this postcondition") — proving the invariant is doing real work.

> **Toolchain notes (the build is fiddly on this machine).**
> - Verus publishes **no Linux-aarch64 binary**, so the toolchain is built from source and
>   installed to `~/.verus` (local disk). `verify.sh` finds it there; set `VERUS_BIN` to override,
>   or drop the upstream `arm64-macos` release into `./tools` on a Mac host.
> - The toolchain must run from **local disk, not this 9p shared mount** — `rust_verify`
>   `dlopen()`s proc-macro `.so`s and 9p can't reliably mmap-execute them (segfault).
> - z3 4.12.5 is built from source too (the upstream arm64 asset is mislabeled x86); its version
>   string carries a build hashcode, so `verify.sh` passes `-V no-solver-version-check`.

## Table-URI discovery

A table is a `Multiaddr` ending in `/p2p/<PeerId>`, wrapped in the `tencentpoker:` scheme, e.g.

```
tencentpoker:/ip4/203.0.113.7/udp/9000/quic-v1/p2p/12D3KooW...
```

Anyone you send that string to can dial directly into the table — no DHT, no rendezvous server.

## Status

Initial scaffold: dependency stack pinned and building, a runnable headless smoke test, and a
verified `core`. Next milestones are the Barnett–Smart mental-poker shuffle, the settlement circuit
(exported to the contract's `Verifier.sol`), the libp2p swarm, and the egui table UI.
