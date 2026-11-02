# Horizon for iPhone and iPad

A native build of the game for your own devices, with touch controls. The app bundles the game
resources prepared from **your own** Minecraft: Java Edition (Mojang's files): never share the app,
upload it or commit what it is built from.

## Once

1. Rust targets: `rustup target add aarch64-apple-ios aarch64-apple-ios-sim`; `brew install xcodegen`.
2. `ios/prepare_resources.sh`: downloads the released launcher (checksum-verified), signs you in with the Microsoft account that
   owns Minecraft (a code to enter in the browser), downloads Minecraft and prepares it into
   `.local/horizon-client/<version>`. The launcher keeps its own data in `.local/ios-launcher`.
   It also downloads Bedrock Edition's touch control textures from Mojang's bedrock-samples
   (`ios/bedrock_controls.sh`, checksum-verified, into `.local/bedrock-controls`): Mojang's files
   too, bundled into the app the same way.
3. `cp ios/Local.xcconfig.example ios/Local.xcconfig` and set your team and a bundle id of yours.

## Build and run

```sh
ios/run.sh            # the booted simulator
ios/run.sh device     # the connected iPhone or iPad
```

Or `cd ios && xcodegen` and open `Horizon.xcodeproj` in Xcode (▶ builds the Rust library too).
Debug builds use Cargo's `fast` profile; Archive uses `release`.

## Signing in in the game

Online-mode servers need the account that owns Minecraft. With `HORIZON_MSA_CLIENT_ID` set in
`Local.xcconfig` (an Azure app registration you may use, with public client flows allowed and
Mojang's Minecraft API approval: [docs/microsoft-account.md](../docs/microsoft-account.md)),
Options › Horizon Settings › Microsoft Account signs in with a code: Open Link opens
microsoft.com/link in Safari with the code on the clipboard. The game keeps the Microsoft refresh
token in the Keychain and signs in again at every start; Sign Out deletes it. Without a client ID
the page is hidden and the game plays offline, as before.

## Resource packs, shader packs and worlds

The game folder is the app's Documents, which the Files app shows as On My iPhone › Horizon:
put a resource pack's `.zip` (or folder) in `resourcepacks`, a shader pack in `shaderpacks` and a
world folder in `saves`, from the Files app, AirDrop (Save to Files), Safari's downloads or a Mac
(Finder › the device › Files › Horizon). Open Pack Folder on the Resource Packs and Shader Packs
screens opens that folder in the Files app; back in the game the list shows what was added.
Builds before this one kept the game in `Library/Application Support/horizon`; the first start
moves it to Documents. On the simulator the folder is in
`xcrun simctl get_app_container booted <bundle id> data`.

Merging upstream: [MERGING.md](MERGING.md).
