# Vendored mental-poker stack (Geometry Research)

This directory holds a vendored, in-progress port of Geometry Research's
Barnett–Smart mental-poker stack. It is **excluded from the cargo workspace**
(`exclude = ["vendor"]` in the root `Cargo.toml`) so an unported snapshot can
never break the main client build. Each crate is built standalone with cargo
from inside its own directory.

> Build note: this repo lives on a 9p mount where the default `target/` dir
> segfaults on proc-macro dlopen. Always build with
> `export CARGO_TARGET_DIR=$HOME/poker-client-target`.

## Provenance

| Vendored crate | Upstream repository | Upstream path |
|---|---|---|
| `proof-essentials/` | https://github.com/geometryresearch/proof-toolbox | `proof-essentials/` |
| `barnett-smart-card-protocol/` | https://github.com/geometryxyz/mental-poker | `barnett-smart-card-protocol/` |

The upstream Starknet curve crate (`proof-toolbox/starknet-curve`) is **not**
vendored: it is replaced by the published `ark-ed-on-bn254` crate (see below).

## License

Both upstream repos are dual-licensed **MIT OR Apache-2.0**. The original
`LICENSE-MIT` and `LICENSE-APACHE` files are preserved in each vendored crate
directory. This vendored copy is used under those terms.

## Port status (Milestone M2)

The code is being ported from its original toolchain to ours:

* **arkworks 0.3 → 0.6** (`ark-ec`, `ark-ff`, `ark-std`, `ark-serialize`,
  `ark-crypto-primitives`).
* **Starknet curve → Baby Jubjub** (`ark-ed-on-bn254` 0.6). Baby Jubjub's base
  field equals the BN254 scalar field, which is why it is the curve chosen for
  the future on-chain settlement circuit.
* **`ark-marlin` removed.** It has no 0.6 release; its only use was
  `ark_marlin::rng::FiatShamirRng`, now reimplemented in-tree at
  `proof-essentials/src/zkp/fiat_shamir_rng.rs` (a Blake2/`digest`-based
  Fiat–Shamir RNG with the same `from_seed` / `absorb` / `Rng` surface).

### What compiles today (phase 1: FOUNDATION)

`proof-essentials` builds and tests green standalone (`cargo test`, 12/12) for
the foundation layer:

* `src/curve.rs` — Baby Jubjub type aliases (`Projective`, `Affine`, `Fr`, `Fq`).
* `src/utils/` — permutation, rand, vector_arithmetic.
* `src/zkp/transcript.rs` and `src/zkp/fiat_shamir_rng.rs`.
* `src/homomorphic_encryption/el_gamal/` — full ElGamal scheme + arithmetic.
* `src/vector_commitment/pedersen/` — full Pedersen vector commitment.

### Not yet ported (phase 2)

These modules are cfg-gated out (`#[cfg(any())]`) in
`proof-essentials/src/zkp/mod.rs` so the foundation compiles in isolation:

* `src/zkp/proofs/` — schnorr_identification, chaum_pedersen_dl_equality.
* `src/zkp/arguments/` — single_value_product, matrix_elements_product,
  hadamard_product, multi_exponentiation, zero_value_bilinear_map, shuffle.

`barnett-smart-card-protocol` is **not yet ported at all**: its Cargo.toml is
retargeted to 0.6 for coherence, but its source (`src/discrete_log_cards/*`)
still uses the arkworks 0.3 API and depends on the gated arguments layer.

Anything gated or deferred is clearly marked with `NOTE (M2 port)` /
`NOT YET PORTED` comments in-tree.
