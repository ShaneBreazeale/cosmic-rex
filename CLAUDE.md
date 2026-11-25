# Working on cosmic-rex

- Commits: never add Claude as co-author or any attribution line. The owner is the sole contributor.
- Backend types stay behind `crates/backend`; the frontend must not depend on rex-rune, rexrs or ghostty-vt.
- Attaching must never create, stop or resize anything. Input is never queued for later.
- Live tests use throwaway sessions only (`cargo run --release -p rex-backend --example live -- ENDPOINT`).
- Don't copy code from cosmic-term (GPL-3.0).
- Check: `cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
