#!/usr/bin/env bash
# Builds fih, installs it at a fixed path signed with a stable identity, and
# (re)starts the background service.
#
# macOS ties the Accessibility grant to the binary's code signature. An ad-hoc
# signed `cargo build` gets a new signature every time, so the grant is lost on
# each rebuild; signing every build with the same certificate and identifier
# keeps it.
#
#   FIH_INSTALL_DIR    where the binary goes (default: ~/.local/bin)
#   FIH_SIGN_IDENTITY  codesigning identity (default: the first
#                      "Apple Development" certificate in the keychain)
set -euo pipefail

# Alfred and launchd start with a bare PATH.
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"

repo="$(cd "$(dirname "$0")/.." && pwd)"
service_id="com.github.csjaugustus.fih"
install_dir="${FIH_INSTALL_DIR:-$HOME/.local/bin}"
dest="$install_dir/fih"
plist="$HOME/Library/LaunchAgents/$service_id.plist"

identity="${FIH_SIGN_IDENTITY:-$(security find-identity -v -p codesigning |
    awk -F'"' '/Apple Development/ { print $2; exit }')}"
if [[ -z "$identity" ]]; then
    echo "warning: no signing certificate found; signing ad hoc, so Accessibility" >&2
    echo "         will have to be granted again after every deploy" >&2
    identity="-"
fi

echo "==> building"
cargo build --release --manifest-path "$repo/Cargo.toml"

echo "==> installing to $dest"
mkdir -p "$install_dir"
# Sign a copy and move it into place: the running daemon keeps the old file
# until it restarts, and the service never sees a half-written binary.
cp "$repo/target/release/fih" "$dest.new"
codesign --force --sign "$identity" --identifier "$service_id" "$dest.new"
mv -f "$dest.new" "$dest"

# The launch agent records the binary's path when it is installed; reinstall
# it when it points somewhere else (e.g. a `cargo install` copy).
if [[ -f "$plist" ]] && ! grep -q "<string>$dest</string>" "$plist"; then
    echo "==> pointing the service at $dest"
    "$dest" uninstall || true
fi

# A copy run straight from the build tree would fight the service over the
# same windows.
if pgrep -f "$repo/target/(debug|release)/fih|^target/(debug|release)/fih" >/dev/null; then
    echo "==> stopping fih started from the build tree"
    pkill -f "$repo/target/(debug|release)/fih|^target/(debug|release)/fih" || true
    sleep 1
fi

if launchctl print "gui/$(id -u)/$service_id" >/dev/null 2>&1; then
    echo "==> restarting"
    "$dest" restart
else
    echo "==> starting"
    "$dest" start
fi
echo "==> fih is running from $dest"
