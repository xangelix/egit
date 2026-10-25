#!/bin/sh
# Builds egit for a browser into target/web (or $1): its WebAssembly, threaded, and the page and
# worker loading it, compressed beside (`.gz`, and `.br` where brotli is installed), as
# `egit-relay --web` serves them.
#
# Threaded WebAssembly takes the nightly `scripts/web-env.sh` pins, with its
# wasm32-unknown-unknown target and rust-src.
#
# wasm-bindgen's CLI must be the version the lock file names (`cargo install wasm-bindgen-cli
# --version <it> --locked`); wasm-opt (binaryen) makes the page smaller where it is installed.
set -eu
cd "$(dirname "$0")/.."
out=${1:-target/web}
. scripts/web-env.sh

# Memory shared by every worker (imported, so each worker is handed the page's), at most 2 GiB;
# the exports wasm-bindgen sets each worker's thread up with.
RUSTFLAGS="$web_rustflags
    -C link-arg=--shared-memory -C link-arg=--import-memory
    -C link-arg=--max-memory=2147418112
    -C link-arg=--export=__heap_base -C link-arg=--export=__wasm_init_tls
    -C link-arg=--export=__tls_size -C link-arg=--export=__tls_align
    -C link-arg=--export=__tls_base" \
    cargo +"$web_toolchain" build -Z build-std=panic_abort,std --package egit-web \
    --target wasm32-unknown-unknown --release --locked

# Where cargo installs it, should it not be on PATH.
if ! command -v wasm-bindgen >/dev/null 2>&1 && [ -x "${CARGO_HOME:-$HOME/.cargo}/bin/wasm-bindgen" ]; then
    PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
fi
rm -rf "$out"
mkdir -p "$out"
wasm-bindgen --target web --no-typescript --out-dir "$out" \
    target/wasm32-unknown-unknown/release/egit_web.wasm
if command -v wasm-opt >/dev/null 2>&1; then
    wasm-opt -O2 --enable-threads --enable-bulk-memory --enable-nontrapping-float-to-int \
        --enable-sign-ext --enable-mutable-globals \
        "$out/egit_web_bg.wasm" -o "$out/egit_web_bg.wasm"
fi
cp crates/egit-web/web/index.html crates/egit-web/web/start.js crates/egit-web/web/worker.js \
    "$out/"
for file in "$out"/*.wasm "$out"/*.js "$out"/*.html; do
    gzip -9 --keep --force "$file"
    if command -v brotli >/dev/null 2>&1; then
        brotli --best --keep --force "$file"
    fi
done
echo "egit's web build is in $out: egit-relay --web $out"
