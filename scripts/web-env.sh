# Sourced by what builds or lints egit for a browser (scripts/build-web.sh,
# scripts/check-platforms.sh, CI): the toolchain its threads need and the flags it compiles with.
#
# Threaded WebAssembly takes a nightly: the standard library is built again with atomics
# (`-Z build-std`), which only nightly allows. It is pinned here (`EGIT_WEB_TOOLCHAIN` names
# another), with its wasm32-unknown-unknown target and rust-src:
#
#     rustup toolchain install nightly-2026-10-10 --component rust-src,clippy \
#         --target wasm32-unknown-unknown
web_toolchain=${EGIT_WEB_TOOLCHAIN:-nightly-2026-10-10}
# Atomics and the memory operations threads share; and the page's randomness for getrandom 0.3
# (egui's hashing). Never the user's own flags: they would change what is built.
web_rustflags='--cfg getrandom_backend="wasm_js" -C target-feature=+atomics,+bulk-memory,+mutable-globals'
unset CARGO_ENCODED_RUSTFLAGS
