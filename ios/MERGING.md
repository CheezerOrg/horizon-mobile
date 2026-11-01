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

## After a merge

1. If the root `Cargo.toml`'s `[patch.crates-io]` or `[profile.*]` changed, copy them into
   `ios/Cargo.toml` (patches apply only from the workspace that builds).
2. `cp Cargo.lock ios/Cargo.lock` when dependencies moved, then
   `cargo build --manifest-path ios/Cargo.toml --target aarch64-apple-ios-sim`.
3. `cargo check -p horizon-client --no-default-features --features audio --target aarch64-apple-ios --lib`
   catches a new desktop-only dependency.
4. If `VANILLA_VERSION` changed, run `ios/prepare_resources.sh` again.
