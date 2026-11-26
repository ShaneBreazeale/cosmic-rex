#!/usr/bin/env bash
# Swap the rex-rune core:
#   update-backend.sh [revision]   take a revision (default: upstream's newest master)
#   update-backend.sh --check      only say whether upstream moved: exit 0 if not, 10 if it did
# Keeps every pin in step, then runs the full check (clippy and all tests). On failure the old
# pins are restored. Set UPDATE_SUMMARY=path to also write a Markdown summary there.
set -euo pipefail
cd "$(dirname "$0")/.."
upstream=https://github.com/shuv1337/rex-rune
summary=${UPDATE_SUMMARY:-/dev/null}
old=$(sed -n 's|.*rex-rune", rev = "\([0-9a-f]\{40\}\)".*|\1|p' Cargo.toml | sort -u)
[ "$(wc -w <<<"$old")" = 1 ] || { echo "Cargo.toml pins more than one rex-rune revision: $old" >&2; exit 1; }

check=false
if [ "${1:-}" = --check ]; then check=true; shift; fi
new=${1:-$(git ls-remote "$upstream" refs/heads/master | cut -f1)}
[[ $new =~ ^[0-9a-f]{40}$ ]] || { echo "Not a full revision: '$new'" >&2; exit 1; }
compare="$upstream/compare/$old...$new"
if [ "$old" = "$new" ]; then echo "Already at $new"; exit 0; fi
if $check; then echo "Upstream moved: $old -> $new   $compare"; exit 10; fi

# A new revision rebuilds Ghostty, which needs Zig 0.17.
[ -x .toolchain/zig/zig ] && export PATH="$PWD/.toolchain/zig:$PATH"
zig_version=$(zig version 2>/dev/null || true)
[[ $zig_version == 0.17.* ]] || { echo "Need Zig 0.17 (found '${zig_version:-none}'): run scripts/fetch-zig.sh" >&2; exit 1; }

backup=$(mktemp -d)
cp Cargo.toml Cargo.lock "$backup/"
restore() {
  cp "$backup/Cargo.toml" "$backup/Cargo.lock" .
  rm -rf "$backup"
  echo "Kept $old: $1" >&2
  printf '\n**Failed:** %s. Kept `%s`.\n' "$1" "$old" >>"$summary"
  exit 1
}
sed -i "s/$old/$new/g" Cargo.toml
echo "$old -> $new   $compare"
{
  echo "rex-rune \`${old:0:12}\` → \`${new:0:12}\`"
  echo
  echo "Upstream changes: $compare"
} >"$summary"

# rex-rune's manifest carries its own pins and patches; show when ours may need to follow.
work="$backup/up"
git init -q "$work"
if git -C "$work" fetch -q --depth 1 "$upstream" "$old" && git -C "$work" branch -q old FETCH_HEAD \
  && git -C "$work" fetch -q --depth 1 "$upstream" "$new"; then
  if pins=$(git -C "$work" diff old FETCH_HEAD -- Cargo.toml | grep -E '^[+-].*(rexrs|ghostty|patch)'); then
    echo "$pins"
    echo "^ upstream changed these: check [workspace.dependencies] and [patch] here"
    printf '\n**Upstream changed its own pins or patches.** Check `[workspace.dependencies]` and `[patch]`:\n\n```diff\n%s\n```\n' "$pins" >>"$summary"
  fi
fi

cargo clippy --workspace --all-targets -- -D warnings || restore "it does not build or clippy fails"
cargo test --workspace || restore "the tests fail"
rm -rf "$backup"
cat >>"$summary" <<'EOF'

Clippy and all tests pass.

- [ ] Read the upstream changes
- [ ] Live test against a throwaway session: `cargo run --release -p rex-backend --example live -- ENDPOINT`
EOF
echo "Updated. Review the compare link, run the live test against a throwaway session:"
echo "  cargo run --release -p rex-backend --example live -- ENDPOINT"
echo "then commit Cargo.toml and Cargo.lock."
