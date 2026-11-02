#!/bin/bash
# Bedrock Edition's touch control textures, which the touch controls draw: downloaded from Mojang's
# bedrock-samples at a pinned revision and checked against bedrock-controls.sha256. They are
# Mojang's files under the Minecraft EULA (their LICENSE.md comes with them): they stay in
# .local/bedrock-controls (gitignored), bundle_resources.sh copies them into the app, and they are
# never committed. Prints the folder they are in.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
revision=46ba6ea985fb5a92d79a9419198f10dda14c199d
sums="$here/bedrock-controls.sha256"
out="$here/../.local/bedrock-controls/$revision"
if [ -d "$out" ] && (cd "$out" && shasum -a 256 -c --quiet "$sums") >/dev/null 2>&1; then
    echo "$out"
    exit 0
fi
part="$out.partial"
rm -rf "$part"
while read -r _ path; do
    mkdir -p "$part/$(dirname "$path")"
    curl -fsSL --proto '=https' -o "$part/$path" \
        "https://raw.githubusercontent.com/Mojang/bedrock-samples/$revision/$path"
done < "$sums"
(cd "$part" && shasum -a 256 -c --quiet "$sums") >&2
rm -rf "$out"
mv "$part" "$out"
echo "$out"
