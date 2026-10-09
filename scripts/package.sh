#!/usr/bin/env bash
# Builds a release binary and packs it with its desktop entry and icon into
# dist/pangram-desktop-<version>-<arch>-linux.tar.gz (plus a .sha256). The archive's top-level
# directory mirrors a prefix, so it can be unpacked straight into ~/.local:
#
#   tar -xzf pangram-desktop-*.tar.gz -C ~/.local --strip-components=1
set -euo pipefail
cd "$(dirname "$0")/.."

version="$(sed -n '/^\[workspace\.package\]/,/^\[/ s/^version = "\(.*\)"/\1/p' Cargo.toml)"
app_id=net.batkin.pangram-desktop
name="pangram-desktop-$version-$(uname -m)-linux"
stage="dist/$name"

cargo build --release --locked -p pangram-desktop

rm -rf "$stage" "dist/$name.tar.gz" "dist/$name.tar.gz.sha256"
install -Dm755 target/release/pangram-desktop "$stage/bin/pangram-desktop"
install -Dm644 "data/$app_id.desktop" "$stage/share/applications/$app_id.desktop"
install -Dm644 "data/$app_id.svg" "$stage/share/icons/hicolor/scalable/apps/$app_id.svg"
install -Dm644 README.md "$stage/share/doc/pangram-desktop/README.md"
install -Dm644 -t "$stage/share/licenses/pangram-desktop" LICENSE-APACHE LICENSE-MIT

tar -C dist --owner=0 --group=0 --sort=name -czf "dist/$name.tar.gz" "$name"
rm -rf "$stage"
(cd dist && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")
echo "dist/$name.tar.gz"
