#!/bin/bash
# Builds a Release Horizon.ipa in ios/build: `ios/ipa.sh` unsigned (for a sideloading tool that
# signs it with your Apple ID: AltStore, Sideloadly...), `ios/ipa.sh signed` archived and exported
# with Local.xcconfig's team (development signing). Extra arguments go to xcodebuild.
# The app holds Mojang's files: the .ipa is for your own devices, never to share or upload.
set -euo pipefail
cd "$(dirname "$0")"
[ -f Local.xcconfig ] || cp Local.xcconfig.example Local.xcconfig
xcodegen --quiet
mode="${1:-unsigned}"
[ $# -gt 0 ] && shift
ipa="build/Horizon.ipa"
rm -f "$ipa"
if [ "$mode" = signed ]; then
    archive="build/Horizon.xcarchive"
    rm -rf "$archive" build/export
    xcodebuild -project Horizon.xcodeproj -scheme Horizon -configuration Release \
        -destination "generic/platform=iOS" -derivedDataPath build -archivePath "$archive" \
        -allowProvisioningUpdates "$@" archive
    options="build/ExportOptions.plist"
    cat > "$options" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>method</key><string>debugging</string>
    <key>signingStyle</key><string>automatic</string>
    <key>compileBitcode</key><false/>
    <key>stripSwiftSymbols</key><true/>
    <key>thinning</key><string>&lt;none&gt;</string>
</dict>
</plist>
EOF
    xcodebuild -exportArchive -archivePath "$archive" -exportPath build/export \
        -exportOptionsPlist "$options" -allowProvisioningUpdates
    mv build/export/Horizon.ipa "$ipa"
elif [ "$mode" = unsigned ]; then
    xcodebuild -project Horizon.xcodeproj -scheme Horizon -configuration Release -sdk iphoneos \
        -destination "generic/platform=iOS" -derivedDataPath build \
        CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO CODE_SIGN_IDENTITY= "$@" build
    rm -rf build/ipa
    mkdir -p build/ipa/Payload
    cp -R build/Build/Products/Release-iphoneos/Horizon.app build/ipa/Payload/
    (cd build/ipa && zip -qry ../Horizon.ipa Payload)
else
    echo "usage: ios/ipa.sh [unsigned|signed] [xcodebuild arguments...]" >&2
    exit 1
fi
echo "$PWD/$ipa ($(du -h "$ipa" | cut -f1))"
