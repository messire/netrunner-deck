#!/usr/bin/env bash
# One-time setup of deck-server on a Linux node (Ubuntu), as the login user.
# Installs the build toolchain (apt packages, rustup, Zig 0.16.0), then hands
# over to update.sh, which builds, installs the systemd user units and starts
# the server. Herdr and its own service are not touched: deck-server keeps its
# own dirs (~/.config/deck-server) and sockets.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
log() { echo "[deck-server-install] $*"; }

log "apt packages"
sudo DEBIAN_FRONTEND=noninteractive apt-get update -qq
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq \
  build-essential pkg-config git curl xz-utils jq ca-certificates >/dev/null

if ! command -v rustup >/dev/null && [ ! -x "$HOME/.cargo/bin/rustup" ]; then
  log "rustup"
  curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none
fi

zig_dir="$HOME/.local/zig-0.16.0"
if [ ! -x "$zig_dir/zig" ]; then
  log "zig 0.16.0"
  arch="$(uname -m)"
  entry="$(curl -fsSL https://ziglang.org/download/index.json | jq -r --arg k "$arch-linux" '."0.16.0"[$k]')"
  url="$(jq -r .tarball <<<"$entry")"
  sum="$(jq -r .shasum <<<"$entry")"
  tmp="$(mktemp -d)"
  curl -fsSL "$url" -o "$tmp/zig.tar.xz"
  echo "$sum  $tmp/zig.tar.xz" | sha256sum -c - >/dev/null
  mkdir -p "$zig_dir"
  tar -xJf "$tmp/zig.tar.xz" -C "$zig_dir" --strip-components=1
  rm -rf "$tmp"
fi

# User services keep running without a login session and start at boot.
sudo loginctl enable-linger "$USER"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"

mkdir -p "$HOME/.local/bin"
install -m 755 "$here/update.sh" "$HOME/.local/bin/deck-server-update"
"$HOME/.local/bin/deck-server-update" --force
systemctl --user enable --now deck-server.service deck-server-update.timer
log "done: $(systemctl --user is-active deck-server.service)"
