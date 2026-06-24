#!/usr/bin/env bash
#
# Formally verify the synchronous core with Verus.
#
#   ./verify.sh            # verify core/src/lib.rs
#   ./verify.sh <file>     # verify a specific file
#   VERUS_BIN=/path/verus ./verify.sh
#
# Toolchain resolution (first that exists wins):
#   1. $VERUS_BIN
#   2. ~/.verus/verus            <- this Linux VM: built from source, installed on LOCAL disk
#   3. ./tools/verus            <- Mac host: drop the upstream arm64-macos release here
#
# Why not run the toolchain from this repo on the VM? This tree lives on a 9p shared
# mount; `rust_verify` dlopen()s proc-macro .so files, which 9p cannot reliably
# mmap-execute (segfault). z3 alone is fine, but the verifier must run from local disk.
#
# `-V no-solver-version-check`: the from-source z3 reports a build hashcode in its version
# string that Verus's exact-match check rejects. The solver is genuine z3 4.12.5. On a Mac
# host using the upstream release, this flag is harmless.
set -euo pipefail
cd "$(dirname "$0")"

FILE="${1:-core/src/lib.rs}"

if [ -n "${VERUS_BIN:-}" ] && [ -x "$VERUS_BIN" ]; then
    VERUS="$VERUS_BIN"
elif [ -x "$HOME/.verus/verus" ]; then
    VERUS="$HOME/.verus/verus"
elif [ -x "./tools/verus" ]; then
    VERUS="./tools/verus"
else
    echo "error: no verus toolchain found (set VERUS_BIN, install to ~/.verus, or add ./tools)" >&2
    exit 1
fi

echo "verifying $FILE with $VERUS"
exec "$VERUS" --crate-type=lib "$FILE" -V no-solver-version-check
