#!/usr/bin/env bash
# Put the Zig that Ghostty builds with in .toolchain/zig (gitignored). Does nothing if it is there.
set -euo pipefail
cd "$(dirname "$0")/.."
version=0.17.1
dest=.toolchain/zig
if [ -x "$dest/zig" ] && [ "$("$dest/zig" version)" = "$version" ]; then
  echo "Zig $version already in $dest"
  exit 0
fi
case "$(uname -m)" in
  x86_64) arch=x86_64 sum=3581863e3cbcfd0fce0efc682ce7e9e93da96668a977791d356a26d2a7e9b17a ;;
  aarch64 | arm64) arch=aarch64 sum=a1546d5646eb184437918d6635ca6b762a0e889eac09cef720ee7a1ed3fe7795 ;;
  *) echo "No pinned Zig for $(uname -m)" >&2; exit 1 ;;
esac
name=zig-$arch-linux-$version
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl -fsSL --retry 3 -o "$tmp/$name.tar.xz" "https://ziglang.org/download/$version/$name.tar.xz"
echo "$sum  $tmp/$name.tar.xz" | sha256sum -c --quiet -
tar -xJf "$tmp/$name.tar.xz" -C "$tmp"
rm -rf "$dest"
mkdir -p "$(dirname "$dest")"
mv "$tmp/$name" "$dest"
echo "Zig $version in $dest"
