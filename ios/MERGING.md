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
  HUD, the stick at the top of `pad_move`, the `touch` control command) and the pointer grab that is
  the game's own on iOS (`capture`).
- **Fork-only files**: `crates/horizon-client-input/src/touch.rs` (the gestures, unit-tested) and
  `crates/horizon-client/src/game/touch.rs` (the game side). Upstream never touches them.
  `gamepad::length` was made `pub` for them.

Test the touch controls on the desktop: launch with `HORIZON_TOUCH=1`, then
`horizon-ctl touch start|move|end ID X Y` (GUI pixels) plays fingers.

## After a merge

1. If the root `Cargo.toml`'s `[patch.crates-io]` or `[profile.*]` changed, copy them into
   `ios/Cargo.toml` (patches apply only from the workspace that builds).
2. `cp Cargo.lock ios/Cargo.lock` when dependencies moved, then
   `cargo build --manifest-path ios/Cargo.toml --target aarch64-apple-ios-sim`.
3. `cargo check -p horizon-client --no-default-features --features audio --target aarch64-apple-ios --lib`
   catches a new desktop-only dependency.
4. If `VANILLA_VERSION` changed, run `ios/prepare_resources.sh` again.

## The patched winit (`ios/third_party/winit`)

winit 0.30.13 with two iOS changes, each marked `horizon-ios:` (`git grep -n "horizon-ios:" ios/third_party`):
the UIScene life cycle UIKit requires of apps built with current SDKs (windows join the app's
`UIWindowScene`), and windows that always fill the screen. It applies only to the ios/ workspace.
When upstream moves to another winit, port these changes to that version (or drop them once winit
has them) and update the path in `ios/Cargo.toml`.
