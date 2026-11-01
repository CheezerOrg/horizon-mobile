# Horizon for iPhone and iPad

A native build of the game for your own devices, with touch controls. The app bundles the game
resources prepared from **your own** Minecraft: Java Edition (Mojang's files): never share the app,
upload it or commit what it is built from.

## Once

1. Rust targets: `rustup target add aarch64-apple-ios aarch64-apple-ios-sim`; `brew install xcodegen`.
2. `ios/prepare_resources.sh`: builds the launcher, signs you in with the Microsoft account that
   owns Minecraft (a code to enter in the browser), downloads Minecraft and prepares it into
   `.local/horizon-client/<version>`. The launcher keeps its own data in `.local/ios-launcher`.
3. `cp ios/Local.xcconfig.example ios/Local.xcconfig` and set your team and a bundle id of yours.

## Build and run

```sh
ios/run.sh            # the booted simulator
ios/run.sh device     # the connected iPhone or iPad
```

Or `cd ios && xcodegen` and open `Horizon.xcodeproj` in Xcode (▶ builds the Rust library too).
Debug builds use Cargo's `fast` profile; Archive uses `release`.

The game keeps its worlds, options and logs in the app's `Library/Application Support/horizon`;
on the simulator: `xcrun simctl get_app_container booted <bundle id> data`.

Merging upstream: [MERGING.md](MERGING.md).
