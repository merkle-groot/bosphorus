#!/usr/bin/env bash
# Builds the Bosphorus Basecamp packages: libbosphorus_ffi with cargo (Metal proving; nixpkgs'
# SDK has no `xcrun metal`), staged into the core module's lib/, then both portable .lgx with nix.
#   basecamp/build.sh            → basecamp/out/{bosphorus,bosphorus_ui}.lgx
set -euo pipefail
cd "$(dirname "$0")/.."
. /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh 2>/dev/null || true
# Flake inputs come through GitHub's API, whose anonymous limit (60/h) a cold build exceeds: use
# the gh CLI's token for this process only, if there is one.
if TOKEN=$(gh auth token 2>/dev/null); then
  export NIX_CONFIG="access-tokens = github.com=$TOKEN"
fi
cargo build --release -p bosphorus-ffi
CORE=basecamp/bosphorus-core
mkdir -p "$CORE/lib/aarch64-darwin" basecamp/out
cp ffi/bosphorus_ffi.h "$CORE/lib/"
cp target/release/libbosphorus_ffi.dylib "$CORE/lib/aarch64-darwin/"
chmod u+w "$CORE/lib/aarch64-darwin/libbosphorus_ffi.dylib"
nix build "path:$PWD/$CORE#lgx-portable" -L --out-link basecamp/out/core
nix build "path:$PWD/basecamp/bosphorus-ui#lgx-portable" -L --out-link basecamp/out/ui \
  --override-input bosphorus "path:$PWD/$CORE"
cp -f basecamp/out/core/*.lgx basecamp/out/bosphorus.lgx
cp -f basecamp/out/ui/*.lgx basecamp/out/bosphorus_ui.lgx
ls -la basecamp/out/*.lgx
