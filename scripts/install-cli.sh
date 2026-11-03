#!/usr/bin/env bash
# Install the headless claude-sync-cli into ~/.local/bin (no sudo).
# Usage: curl -fsSL https://raw.githubusercontent.com/MehdiMamas/claude-sync/main/scripts/install-cli.sh | bash
# Optional: VERSION=0.1.22 to pin a release, BIN_DIR=... to change the folder.
set -euo pipefail

repo="MehdiMamas/claude-sync"
bin_dir="${BIN_DIR:-$HOME/.local/bin}"

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) triple=x86_64-unknown-linux-gnu ;;
  Linux-aarch64 | Linux-arm64) triple=aarch64-unknown-linux-gnu ;;
  Darwin-x86_64) triple=x86_64-apple-darwin ;;
  Darwin-arm64 | Darwin-aarch64) triple=aarch64-apple-darwin ;;
  *) echo "no claude-sync-cli release for $(uname -s) $(uname -m)" >&2; exit 1 ;;
esac

if [ -n "${VERSION:-}" ]; then
  tag="v${VERSION#v}"
else
  # The public releases page redirects /releases/latest to the newest tag.
  url=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$repo/releases/latest")
  tag="${url##*/}"
fi
case "$tag" in v[0-9]*) ;; *) echo "could not find the latest release (got '$tag')" >&2; exit 1 ;; esac

name="claude-sync-cli-$triple.tar.gz"
base="https://github.com/$repo/releases/download/$tag"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "Downloading $name ($tag)"
curl -fsSL -o "$tmp/$name" "$base/$name"
curl -fsSL -o "$tmp/$name.sha256" "$base/$name.sha256"

want=$(awk '{print $1}' "$tmp/$name.sha256")
if command -v sha256sum >/dev/null; then
  got=$(sha256sum "$tmp/$name" | awk '{print $1}')
else
  got=$(shasum -a 256 "$tmp/$name" | awk '{print $1}')
fi
if [ "$want" != "$got" ]; then
  echo "checksum mismatch for $name: expected $want, got $got; nothing installed" >&2
  exit 1
fi

tar -C "$tmp" -xzf "$tmp/$name" claude-sync-cli
mkdir -p "$bin_dir"
install -m 0755 "$tmp/claude-sync-cli" "$bin_dir/claude-sync-cli.new"
mv -f "$bin_dir/claude-sync-cli.new" "$bin_dir/claude-sync-cli"
echo "Installed $bin_dir/claude-sync-cli"
case ":$PATH:" in *":$bin_dir:"*) ;; *) echo "Add $bin_dir to your PATH to run it by name." ;; esac
echo "Next: claude-sync-cli login, then claude-sync-cli join <owner/repo>, then claude-sync-cli service install"
