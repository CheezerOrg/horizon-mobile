#!/bin/bash
# Xcode's "Rust" build phase: builds ios/ (the game as a static library) for the platform being
# built and copies it where project.yml's LIBRARY_SEARCH_PATHS looks. Debug builds use the
# `fast` profile (an unoptimized game is unplayable), Release the release profile.
set -euo pipefail
source "$HOME/.cargo/env" 2>/dev/null || true
cd "$(dirname "$0")"
case "${PLATFORM_NAME:-iphonesimulator}" in
    iphoneos) target=aarch64-apple-ios ;;
    iphonesimulator) target=aarch64-apple-ios-sim ;;
    *) echo "error: unsupported platform ${PLATFORM_NAME}" >&2; exit 1 ;;
esac
if [ "${CONFIGURATION:-Debug}" = Release ]; then profile=release; else profile=fast; fi
# Xcode's SDKROOT would point the build scripts' (host) C compilers at the iOS SDK.
env -u SDKROOT -u LIBRARY_PATH IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-16.0}" \
    cargo build --manifest-path Cargo.toml --target "$target" --profile "$profile"
out="target/xcode/${PLATFORM_NAME:-iphonesimulator}"
mkdir -p "$out"
cp -c "target/$target/$profile/libhorizon_ios.a" "$out/libhorizon_ios.a" 2>/dev/null \
    || cp "target/$target/$profile/libhorizon_ios.a" "$out/libhorizon_ios.a"
