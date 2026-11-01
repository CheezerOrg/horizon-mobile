#!/bin/bash
# Builds Horizon.app and runs it: `ios/run.sh` on the booted simulator, `ios/run.sh device` on the
# connected iPhone or iPad (signed with Local.xcconfig's team). Extra arguments go to xcodebuild.
set -euo pipefail
cd "$(dirname "$0")"
[ -f Local.xcconfig ] || cp Local.xcconfig.example Local.xcconfig
xcodegen --quiet
where="${1:-simulator}"
[ $# -gt 0 ] && shift
if [ "$where" = device ]; then
    sdk=iphoneos; destination="generic/platform=iOS"
else
    sdk=iphonesimulator; destination="generic/platform=iOS Simulator"
fi
xcodebuild -project Horizon.xcodeproj -scheme Horizon -configuration Debug -sdk "$sdk" \
    -destination "$destination" -derivedDataPath build -allowProvisioningUpdates "$@" build
app="build/Build/Products/Debug-$sdk/Horizon.app"
id="$(/usr/libexec/PlistBuddy -c 'Print CFBundleIdentifier' "$app/Info.plist")"
if [ "$where" = device ]; then
    device="$(xcrun devicectl list devices --hide-default-columns --columns Identifier --hide-headers \
        --filter "State == 'connected'" | head -1 | tr -d ' ')"
    [ -n "$device" ] || { echo "No connected device." >&2; exit 1; }
    xcrun devicectl device install app --device "$device" "$app"
    xcrun devicectl device process launch --device "$device" "$id"
else
    xcrun simctl install booted "$app"
    xcrun simctl launch --console-pty booted "$id"
fi
