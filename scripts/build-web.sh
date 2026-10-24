#!/bin/sh
# Builds egit for a browser into target/web (or $1): its WebAssembly and the page loading it,
# compressed beside (`.gz`, and `.br` where brotli is installed), as `egit-relay --web` serves
# them. Needs the wasm32-unknown-unknown target and wasm-bindgen's CLI of the version the lock
# file names (`cargo install wasm-bindgen-cli --version <it> --locked`); wasm-opt (binaryen) makes
# it smaller where it is installed.
set -eu
cd "$(dirname "$0")/.."
out=${1:-target/web}

# The page's randomness for getrandom 0.3 (egui's hashing); and no RUSTFLAGS of the user's
# (a threaded build's, say), which this build doesn't take.
RUSTFLAGS='--cfg getrandom_backend="wasm_js"' \
    cargo build --package egit-web --target wasm32-unknown-unknown --release --locked

# Where cargo installs it, should it not be on PATH.
if ! command -v wasm-bindgen >/dev/null 2>&1 && [ -x "${CARGO_HOME:-$HOME/.cargo}/bin/wasm-bindgen" ]; then
    PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
fi
rm -rf "$out"
mkdir -p "$out"
wasm-bindgen --target web --no-typescript --out-dir "$out" \
    target/wasm32-unknown-unknown/release/egit_web.wasm
if command -v wasm-opt >/dev/null 2>&1; then
    wasm-opt -O2 --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
        --enable-mutable-globals "$out/egit_web_bg.wasm" -o "$out/egit_web_bg.wasm"
fi
cp crates/egit-web/web/index.html crates/egit-web/web/start.js "$out/"
for file in "$out"/*.wasm "$out"/*.js "$out"/*.html; do
    gzip -9 --keep --force "$file"
    if command -v brotli >/dev/null 2>&1; then
        brotli --best --keep --force "$file"
    fi
done
echo "egit's web build is in $out: egit-relay --web $out"
