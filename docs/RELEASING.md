# Releasing

Two tracks, kept apart:

- **Backend changes** are taken by hand, when you decide: `scripts/update-backend.sh --check` says whether upstream rex-rune moved, and `scripts/update-backend.sh [rev]` takes it, running clippy and all tests and putting the old pins back if they fail. Read the upstream changes, run the live test against a throwaway session (`cargo run --release -p rex-backend --example live -- ENDPOINT`), then commit `Cargo.toml` and `Cargo.lock`.
- **The app** is released only when you choose. A backend change ships with the next release.

## Cutting a release

1. Set `version` in `[workspace.package]` in the root `Cargo.toml`, run `cargo check` so `Cargo.lock` follows, and commit.
2. `git tag vX.Y.Z && git push origin vX.Y.Z`. Use `vX.Y.Z-rc1` for a pre-release.
3. The "Release" workflow checks that the tag matches the version, runs clippy and the tests, builds, and makes a **draft** release with `cosmic-rex-vX.Y.Z-x86_64-linux.tar.gz`, `cosmic-rex_X.Y.Z-1_amd64.deb` (a pre-release's is `X.Y.Z~rc1-1`, which sorts before `X.Y.Z-1`) and a `.sha256` for each.
4. Edit the draft's notes, download the `.deb`, `sudo apt install ./cosmic-rex_*.deb`, start it from the launcher and run `cosmic-rex --demo` once, then publish.

To build the `.deb` locally: `cargo install cargo-deb --locked`, then `cargo deb -p cosmic-rex --locked`; it lands in `target/debian/`. Its contents and dependencies are under `[package.metadata.deb]` in `crates/cosmic/Cargo.toml`.

If the workflow fails, fix it on main, then move the tag: `git tag -f vX.Y.Z && git push -f origin vX.Y.Z` (delete any half-made draft first).
