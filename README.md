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
cargo build                      # builds wallet + crypto + net + game + deal + protocol + app
cargo test                       # full test suite (incl. real-network trustless-hand tests)
cargo run -p poker-app           # headless smoke: prints a wallet address, a table URI, BN254 check
cargo run -p poker-app --features gui -- --gui   # launch the egui desktop window (needs a display)
cargo run -p poker-app -- host                   # headless CLI host (bot player)
cargo run -p poker-app -- join <tcpoker://…>     # headless CLI join (bot player)
```

The headless smoke output looks like:

```
wallet : account 0 address = 0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266
net    : table URI         = tcpoker://3xZ8q…              (one opaque base58 token)
crypto : BN254 scalar field = 254 bits (g·42 computed)
```

## Playing a networked game (GUI)

Launch the desktop client on each machine and **Host** or **Join** from the lobby:

```bash
cargo run -p poker-app --features gui -- --gui
```

- **Host:** pick the number of players (e.g. 3), keep *Trustless dealing* on, click **Host**, then
  copy the `tcpoker://…` URI and send it to the other players. macOS will prompt to **Allow**
  incoming connections the first time — accept it.
- **Join:** paste the host's `tcpoker://…` URI and click **Join**. (Accept the firewall prompt too.)

Once everyone is seated the host deals; each player sees their own hole cards (no one else can —
the deal is trustless), the shared board and pot, and acts on their turn (fold/check/call/bet/raise).

### Same-LAN vs remote

- **Same Wi-Fi/LAN:** works out of the box. The host's URI carries its private (`192.168.x.x`)
  address; guests dial it directly (mDNS may also auto-discover). Just copy/paste the URI.
- **Remote / over the internet:** only the **host** needs to be reachable from outside (guests dial
  out). The reliable options, in order:
  1. **Public IP / VPS host** — the host's URI carries its public address directly; nothing else to do.
  2. **UPnP-capable router** — the host maps a port and learns its public address automatically; the
     URI is dialable as-is.
  3. **Manual port-forward** — set a fixed *Listen port* in the host lobby, forward that TCP+UDP
     port on the router to the host machine, and share the URI.

  If the host's best address is not internet-routable (e.g. behind CGNAT with no UPnP/forward), the
  lobby shows a ⚠ reachability warning — pick a different host (public IP / UPnP) in that case. There
  is no relay/TURN fallback.

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

A table's dialable `Multiaddr`(s) (each ending in `/p2p/<PeerId>`) are packed into one compact,
base58-encoded `tcpoker://` token — a single copy-paste-safe string with no `/` or whitespace for
chat clients to mangle, e.g.

```
tcpoker://3xZ8qF7…WkP2
```

Anyone you send that string to can dial directly into the table — no DHT, no rendezvous server.

## Status

The P2P stack is end-to-end functional: trustless (Barnett–Smart mental-poker) dealing over libp2p,
the replicated betting state machine, and an egui desktop client for human play. The trustless deal
rounds use reliable request-response delivery, so networked hands complete without the former
showdown stall (see `KNOWN_ISSUES.md`). Remaining work: the Groth16 settlement circuit (exported to
the contract's `Verifier.sol`) and on-chain buy-in/settlement.
