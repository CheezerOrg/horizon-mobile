# Keeping the iOS port merged with upstream

The port lives on the fork's `ios` branch; `main` mirrors horizon-rust/horizon. Bring upstream in
with a merge (not a rebase), so each conflict is resolved once:

```sh
git fetch upstream && git checkout ios && git merge upstream/main
```

## What the port changes outside `ios/`

- **Portability fixes** (Metal-only on every Apple OS, arboard gated off iOS, `data_dir`,
  `resident_bytes`, `learn_layout`, the open-file limit). They are correct for desktop too and are
  meant for upstream PRs; once one is merged upstream it stops differing here.
- **`// ios-port:` hooks**: the few lines in upstream files the iOS app needs. List them with
  `git grep -n "ios-port:"` after every merge and check each still sits where it should.
  Today they are the touch controls' wiring (the `touch` module and field in `game.rs`, the
  `WindowEvent::Touch` dispatch, `touch_frame` before `poll_pads`, `draw_touch` at the end of the
  HUD, the touch-held sneak and sprint spared in `heal_hold_latches`, the stick at the top of `pad_move`, the `touch` control command) and the pointer grab that is
  the game's own on iOS (`capture`).
- **The Touch Controls page** (`// ios-port:` too): `Opt::Touch` in `settings.rs`, one arm per
  `Opt` method in `options_ui.rs` (caption, kind, label, value, slider, cycle), `Page::Touch` (its
  title, tooltip and rows, and the buttons that open it on the Controls and Horizon Settings pages),
  `options touch` in `game/control.rs`, and the `touch` field of `PadSettings` in `gamepad.rs`
  (loaded and saved beside the controller's file, as `horizon-touch.txt`). Its strings are a block
  of `horizon.pages.touch*` / `horizon.options.touch.*` keys in `assets/horizon/lang/en_us.json`.
- **The Microsoft Account page** (`// ios-port:`): `Page::Account` and `Act::Account` in
  `options_ui.rs` (title, tooltip, rows, both dispatch arms, the button at the end of Horizon
  Settings), `options account` in `game/control.rs`, `Options::player_name` preferring the
  signed-in account in `game.rs`, and `auth.rs`'s session in a lock (`set_current` can replace it,
  `clear_current` signs out) instead of a `OnceLock`. Its strings are the `horizon.pages.account*`
  / `horizon.account.*` keys after the touch block in `en_us.json`. The flow itself is the
  fork-only `crates/horizon-client-render/src/msa.rs` (a port of `launcher/src/account.rs`'s
  device-code sign-in: if upstream changes those requests, follow them there), re-exported in
  `client_lib.rs`; `ios/src/account.rs` gives it the Keychain and Safari.
- **Fork-only files**: `crates/horizon-client-input/src/touch.rs` (the gestures and
  `TouchSettings`, unit-tested) and `crates/horizon-client/src/game/touch.rs` (the game side).
  Upstream never touches them. `gamepad::length` was made `pub` for them.
- **Bedrock Edition's control textures**: the touch controls draw them from
  `textures/gui/bedrock` in the resources. They are Mojang's files (bedrock-samples, under the
  Minecraft EULA) and are never committed: `ios/bedrock_controls.sh` downloads the files pinned in
  `ios/bedrock-controls.sha256` into `.local/bedrock-controls`, and `bundle_resources.sh` copies
  them into the app. Without them the controls draw plain shapes. To see them on the desktop, put
  them in a resource pack under `assets/minecraft/textures/gui/bedrock`.

Test the touch controls on the desktop: launch with `HORIZON_TOUCH=1`, then
`horizon-ctl touch start|move|end ID X Y` (GUI pixels) plays fingers and `horizon-ctl options touch`
opens the settings. Run the game from a scratch working directory: it writes `options.txt` and
`horizon-touch.txt` there.

## After a merge

1. If the root `Cargo.toml`'s `[patch.crates-io]` or `[profile.*]` changed, copy them into
   `ios/Cargo.toml` (patches apply only from the workspace that builds).
2. `cp Cargo.lock ios/Cargo.lock` when dependencies moved, then
   `cargo build --manifest-path ios/Cargo.toml --target aarch64-apple-ios-sim`.
3. `cargo check -p horizon-client --no-default-features --features audio --target aarch64-apple-ios --lib`
   catches a new desktop-only dependency.
4. If `VANILLA_VERSION` changed, run `ios/prepare_resources.sh` again.

## The patched winit (`ios/third_party/winit`)

winit 0.30.13 with these iOS changes, each marked `horizon-ios:` (`git grep -n "horizon-ios:" ios/third_party`):
the UIScene life cycle UIKit requires of apps built with current SDKs (windows join the app's
`UIWindowScene`), windows that always fill the screen, and typing with the on-screen keyboard:
Return is the Enter key, no autocapitals, corrections or smart quotes (they would break commands),
and the view shrinks to end where the keyboard begins, so the text box stays in sight. winit
itself shows the keyboard while the game allows an input method (`set_ime_allowed`, which the game
turns on while a text box has focus). It applies only to the ios/ workspace.
When upstream moves to another winit, port these changes to that version (or drop them once winit
has them) and update the path in `ios/Cargo.toml`.
