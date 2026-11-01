#!/bin/bash
# Xcode's "Bundle resources" build phase: copies the prepared game resources into the app as
# Horizon.app/resources, where the game looks beside its executable (game.rs Options::parse).
# They are Mojang's files: the app is for your own devices, never to share.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
version="$(sed -n 's/^pub const VANILLA_VERSION: &str = "\(.*\)";/\1/p' "$here/../crates/horizon-core/src/version.rs")"
src="${HORIZON_IOS_RESOURCES:-$here/../.local/horizon-client/$version}"
if [ ! -f "$src/horizon-resources.json" ] && [ -n "${HORIZON_IOS_NO_RESOURCES:-}" ]; then
    # A build test only: the app starts and reports the missing resources.
    echo "warning: HORIZON_IOS_NO_RESOURCES is set: the app has no game resources"
    exit 0
fi
if [ ! -f "$src/horizon-resources.json" ]; then
    echo "error: no prepared resources in $src: run ios/prepare_resources.sh first" >&2
    exit 1
fi
dest="${TARGET_BUILD_DIR:?}/${CONTENTS_FOLDER_PATH:?}/resources"
mkdir -p "$dest"
rsync -a --delete "$src/" "$dest/"
