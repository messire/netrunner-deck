#!/usr/bin/env bash
# Install deck-server on a Linux node as the login user, from the rolling
# `deck-server-latest` release, and start it with the machine (systemd user
# unit). After that the server updates itself (src/deck_server/self_update.rs).
# Herdr and its service are not touched: deck-server has its own dirs and sockets.
set -euo pipefail
SOURCE="${DECK_SERVER_SOURCE:-https://github.com/messire/netrunner-deck/releases/download/deck-server-latest}"
here="$(cd "$(dirname "$0")" && pwd)"
arch="$(uname -m)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

curl -fsSL "$SOURCE/deck-server.json" -o "$tmp/deck-server.json"
name="deck-server-linux-$arch"
want="$(grep -o "\"$arch\":{[^}]*}" "$tmp/deck-server.json" | grep -o '"sha256":"[0-9a-f]*"' | cut -d'"' -f4)"
curl -fsSL "$SOURCE/$name" -o "$tmp/deck-server"
echo "$want  $tmp/deck-server" | sha256sum -c - >/dev/null

mkdir -p "$HOME/.local/bin" "${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
install -m 755 "$tmp/deck-server" "$HOME/.local/bin/deck-server"
install -m 644 "$here/deck-server.service" "${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/deck-server.service"
loginctl enable-linger "$USER" 2>/dev/null || sudo loginctl enable-linger "$USER"
systemctl --user daemon-reload
systemctl --user enable --now deck-server.service
"$HOME/.local/bin/deck-server" status
