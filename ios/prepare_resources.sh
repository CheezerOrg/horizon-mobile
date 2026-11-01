#!/bin/bash
# Gets the game resources the iOS app bundles, through the launcher's own download: it signs in
# with the Microsoft account that owns Minecraft: Java Edition, downloads Minecraft and Mojang's
# Java for it, and prepares them. The result lands in .local/horizon-client/<version>, where
# bundle_resources.sh reads it. Run once, and again when the vanilla version changes.
#
# The launcher runs with its own home in .local/ios-launcher (gitignored), apart from your real
# launcher's data. The account's tokens are in your keychain, as the launcher keeps them.
set -euo pipefail
source "$HOME/.cargo/env" 2>/dev/null || true
repo="$(cd "$(dirname "$0")/.." && pwd)"
version="$(sed -n 's/^pub const VANILLA_VERSION: &str = "\(.*\)";/\1/p' "$repo/crates/horizon-core/src/version.rs")"
out="$repo/.local/horizon-client/$version"

export HORIZON_LAUNCHER_HOME="$repo/.local/ios-launcher"
export HORIZON_LAUNCHER_NO_SELF_UPDATE=1
mkdir -p "$HORIZON_LAUNCHER_HOME"

echo "==> Building the launcher (its command line)"
cargo build --release --manifest-path "$repo/launcher/Cargo.toml"
launcher="$repo/launcher/target/release/horizon-launcher"

# One JSON field from the launcher's --json output.
field() { python3 -c "import json,sys; v = json.load(sys.stdin)$1; print('' if v is None else v)"; }

if [ "$("$launcher" --json account list | field "['accounts'].__len__()")" = 0 ]; then
    echo "==> Sign in with the Microsoft account that owns Minecraft: Java Edition"
    "$launcher" account sign-in
fi

if [ -z "$("$launcher" --json versions --installed | field "['selected']")" ]; then
    echo "==> Installing the latest Horizon release (for its preparer)"
    "$launcher" install latest
fi

echo "==> Downloading Minecraft $version from Mojang and preparing it"
"$launcher" prepare --download
tag="$("$launcher" --json versions --installed | field "['selected']")"
prepared="$HORIZON_LAUNCHER_HOME/versions/$tag/.local/horizon-client/$version"

echo "==> Checking the resources against this checkout's preparer"
prepare() { cargo run --release --quiet -p horizon-prepare --manifest-path "$repo/Cargo.toml" -- "$@"; }
if [ -f "$prepared/horizon-resources.json" ] && prepare --check --root "$repo" --output "$prepared"; then
    mkdir -p "$out"
    rsync -a --delete "$prepared/" "$out/"
else
    # The release's preparer is older or newer than this checkout's: prepare again with this one,
    # from the files the launcher downloaded (Minecraft, its server jar and Mojang's Java).
    echo "==> Preparing again with this checkout's horizon-prepare"
    prepare --prepare --complete --root "$repo" \
        --minecraft-dir "$HORIZON_LAUNCHER_HOME/minecraft" \
        --server-jar "$HORIZON_LAUNCHER_HOME/server-jars/$version.jar"
fi
echo "Resources ready in $out ($(du -sh "$out" | cut -f1))."
