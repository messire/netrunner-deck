#!/usr/bin/env bash
# deck-server self-update for a Linux node (systemd user units).
#
# Fetches the tracked branch of the source repo, and when its commit differs
# from the installed one, builds `deck-server`, swaps the binary and restarts
# the service (agents come back through `claude --resume`).
#
#   deck-server-update           update if the branch moved
#   deck-server-update --force   rebuild and restart anyway
#
# Settings live in ~/.config/deck-server/update.env (all optional):
#   DECK_REPO=https://github.com/messire/netrunner-deck.git   # or a local path
#   DECK_BRANCH=main
# deck-server is the `server/` crate of that repository.
set -euo pipefail

CONF="${XDG_CONFIG_HOME:-$HOME/.config}/deck-server/update.env"
DECK_REPO="https://github.com/messire/netrunner-deck.git"
DECK_BRANCH="main"
# shellcheck disable=SC1090
[ -f "$CONF" ] && . "$CONF"

DATA="${XDG_DATA_HOME:-$HOME/.local/share}/deck-server-node"
SRC="$DATA/src"
BIN="$HOME/.local/bin/deck-server"
SELF="$HOME/.local/bin/deck-server-update"
UNITS="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
STAMP="$DATA/installed-commit"
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$HOME/.local/zig-0.16.0:$PATH"
export ZIG="${ZIG:-$HOME/.local/zig-0.16.0/zig}"

mkdir -p "$DATA" "$(dirname "$BIN")" "$UNITS"
exec 9>"$DATA/update.lock"
flock -n 9 || { echo "another update is running"; exit 0; }

log() { echo "[deck-server-update] $*"; }

if [ ! -d "$SRC/.git" ]; then
  log "cloning $DECK_REPO ($DECK_BRANCH)"
  git clone --quiet --branch "$DECK_BRANCH" "$DECK_REPO" "$SRC"
fi
cd "$SRC"
git remote set-url origin "$DECK_REPO"
git fetch --quiet origin "$DECK_BRANCH"
target="$(git rev-parse FETCH_HEAD)"
installed="$(cat "$STAMP" 2>/dev/null || true)"

if [ "$target" = "$installed" ] && [ "${1:-}" != "--force" ] && [ -x "$BIN" ]; then
  exit 0
fi

log "building ${target:0:12} (installed: ${installed:0:12})"
git checkout --quiet --force --detach "$target"
cd server
HERDR_BUILD_COMMIT="$target" cargo build --release --locked --bin deck-server

install -m 755 target/release/deck-server "$BIN.new"
mv -f "$BIN.new" "$BIN"
here="packaging/deck-server-linux"
install -m 755 "$here/update.sh" "$SELF.new"
mv -f "$SELF.new" "$SELF"
for unit in deck-server.service deck-server-update.service deck-server-update.timer; do
  install -m 644 "$here/$unit" "$UNITS/$unit"
done
systemctl --user daemon-reload
echo "$target" > "$STAMP"

log "installed $("$BIN" --version | head -1), restarting"
systemctl --user restart deck-server.service
