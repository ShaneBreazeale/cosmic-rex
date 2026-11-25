#!/usr/bin/env bash
# Swap the rex-rune core: update-backend.sh [revision]   (default: upstream's newest master)
# Keeps every pin in step, then builds and tests. On failure the old pins are restored.
set -euo pipefail
cd "$(dirname "$0")/.."
upstream=https://github.com/shuv1337/rex-rune
old=$(sed -n 's|.*rex-rune", rev = "\([0-9a-f]\{40\}\)".*|\1|p' Cargo.toml | sort -u)
[ "$(wc -w <<<"$old")" = 1 ] || { echo "Cargo.toml pins more than one rex-rune revision: $old" >&2; exit 1; }
new=${1:-$(git ls-remote "$upstream" refs/heads/master | cut -f1)}
[[ $new =~ ^[0-9a-f]{40}$ ]] || { echo "Not a full revision: '$new'" >&2; exit 1; }
[ "$old" != "$new" ] || { echo "Already at $new"; exit 0; }
backup=$(mktemp -d)
cp Cargo.toml Cargo.lock "$backup/"
restore() { cp "$backup/Cargo.toml" "$backup/Cargo.lock" .; rm -rf "$backup"; echo "Kept $old: $1" >&2; exit 1; }
sed -i "s/$old/$new/g" Cargo.toml
echo "$old -> $new   $upstream/compare/$old...$new"
# rex-rune's manifest carries its own pins and patches; show when ours may need to follow.
work="$backup/up"
git init -q "$work"
if git -C "$work" fetch -q --depth 1 "$upstream" "$old" && git -C "$work" branch -q old FETCH_HEAD \
  && git -C "$work" fetch -q --depth 1 "$upstream" "$new"; then
  git -C "$work" diff old FETCH_HEAD -- Cargo.toml | grep -E '^[+-].*(rexrs|ghostty|patch)' \
    && echo "^ upstream changed these: check [workspace.dependencies] and [patch] here" || true
fi
cargo test -p rex-backend || restore "the backend adapter does not build or its contract tests fail"
cargo test -p cosmic-rex || restore "the frontend does not build or its tests fail"
rm -rf "$backup"
echo "Updated. Review the compare link, then commit Cargo.toml and Cargo.lock."
