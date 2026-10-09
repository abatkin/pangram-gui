#!/usr/bin/env bash
# Builds a release binary and installs it with its desktop entry and icon for the current user.
#
#   scripts/install-local.sh              # install into ~/.local
#   scripts/install-local.sh --uninstall  # remove those files (history and settings are kept)
#   PREFIX=/some/dir scripts/install-local.sh
set -euo pipefail
cd "$(dirname "$0")/.."

prefix="${PREFIX:-$HOME/.local}"
app_id=net.batkin.pangram-desktop
bin="$prefix/bin/pangram-desktop"
desktop="$prefix/share/applications/$app_id.desktop"
icon="$prefix/share/icons/hicolor/scalable/apps/$app_id.svg"

refresh() {
  update-desktop-database -q "$prefix/share/applications" 2>/dev/null || true
  gtk-update-icon-cache -q -t "$prefix/share/icons/hicolor" 2>/dev/null || true
  command -v kbuildsycoca6 >/dev/null && kbuildsycoca6 --noincremental >/dev/null 2>&1 || true
}

if [[ "${1:-}" == "--uninstall" ]]; then
  rm -f "$bin" "$desktop" "$icon"
  refresh
  echo "Removed. Settings and history remain in ~/.config/pangram-desktop and ~/.local/share/pangram-desktop."
  exit 0
fi

cargo build --release --locked -p pangram-desktop
install -Dm755 target/release/pangram-desktop "$bin"
install -Dm644 "data/$app_id.svg" "$icon"
install -d "$(dirname "$desktop")"
sed "s|^Exec=.*|Exec=$bin|" "data/$app_id.desktop" > "$desktop"
chmod 644 "$desktop"
refresh
echo "Installed $bin"
echo "Launch \"Pangram\" from the application menu, or run: $bin"
