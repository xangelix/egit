#!/usr/bin/env bash
# Lints egit for macOS and Windows from Linux, every target (tests too), and for a browser (its
# libraries: the tests run git here), as CI's lint does:
#
#     scripts/check-platforms.sh
#
# Nothing is linked or run: that is CI's (`.github/workflows/ci.yml`, the platforms job). Needs
# the pinned toolchain's targets (`rustup target add x86_64-pc-windows-gnu aarch64-apple-darwin
# wasm32-unknown-unknown`), mingw-w64's gcc for Windows' C dependencies, and zig for macOS', whose
# libc headers it carries.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
tools="$root/target/cross-tools"
mkdir -p "$tools"

# zig names targets its own way: drop the `--target` cc-rs passes, and give zig's.
cat > "$tools/cc-aarch64-macos" <<'EOF'
#!/bin/sh
for arg do
  shift
  case "$arg" in --target=*) ;; *) set -- "$@" "$arg" ;; esac
done
exec zig cc -target aarch64-macos "$@"
EOF
printf '#!/bin/sh\nexec zig ar "$@"\n' > "$tools/ar"
chmod +x "$tools/cc-aarch64-macos" "$tools/ar"

cd "$root"
echo "== Windows (x86_64-pc-windows-gnu)"
cargo clippy --workspace --all-targets --all-features --locked \
    --target x86_64-pc-windows-gnu -- -D warnings

echo "== macOS (aarch64-apple-darwin)"
CC_aarch64_apple_darwin="$tools/cc-aarch64-macos" AR_aarch64_apple_darwin="$tools/ar" \
    cargo clippy --workspace --all-targets --all-features --locked \
    --target aarch64-apple-darwin -- -D warnings

echo "== A browser (wasm32-unknown-unknown)"
# The page's randomness for getrandom 0.3, as scripts/build-web.sh builds it.
RUSTFLAGS='--cfg getrandom_backend="wasm_js"' \
    cargo clippy --locked --target wasm32-unknown-unknown -p egit-web -p egit -p egit-git \
    -p egit-forge -p egit-agents -p egit-gix -p egit-graph -p egit-text -- -D warnings
