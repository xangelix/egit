#!/usr/bin/env bash
# Builds a git release into target/git/<version>/ for testing egit against it:
#
#     scripts/build-git.sh 2.43.0
#     EGIT_TEST_GIT=target/git/2.43.0/bin/git cargo test --workspace
#
# 2.43.0 is the oldest git egit supports (Ubuntu 24.04 LTS). Needs a C toolchain, make, and the
# development files of zlib, curl, and expat.
set -euo pipefail

version="${1:?usage: scripts/build-git.sh <version>}"
root="$(cd "$(dirname "$0")/.." && pwd)"
prefix="$root/target/git/$version"
work="$root/target/git/src"

if [[ -x "$prefix/bin/git" ]]; then
    echo "$prefix/bin/git already exists"
    exit 0
fi

mkdir -p "$work"
archive="$work/git-$version.tar.xz"
if [[ ! -f "$archive" ]]; then
    curl --proto '=https' --tlsv1.2 -fsSL \
        "https://mirrors.edge.kernel.org/pub/software/scm/git/git-$version.tar.xz" -o "$archive"
fi
rm -rf "$work/git-$version"
tar -xJf "$archive" -C "$work"

# Releases before C23 use its new keywords (`unreachable`, `bool`) as names: build them as C17.
make -C "$work/git-$version" -j"$(nproc)" \
    prefix="$prefix" CFLAGS="-O2 -std=gnu17" \
    NO_GETTEXT=YesPlease NO_TCLTK=YesPlease NO_PERL=YesPlease NO_PYTHON=YesPlease \
    all install >/dev/null
"$prefix/bin/git" --version
