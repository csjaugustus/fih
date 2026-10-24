#!/usr/bin/env bash
# Builds dist/fih.app and dist/fih-<version>.dmg. With --publish, also uploads
# the DMG to a GitHub release tagged v<version>.
#
# The app is signed with the local Apple Development certificate but not
# notarized (that needs the paid developer program), so on first open macOS
# asks the user to allow it in System Settings → Privacy & Security.
#
#   FIH_SIGN_IDENTITY  codesigning identity (default: the first
#                      "Apple Development" certificate in the keychain)
set -euo pipefail

export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"

repo="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo"

publish=false
case "${1:-}" in
"") ;;
--publish) publish=true ;;
*)
    echo "usage: $0 [--publish]" >&2
    exit 2
    ;;
esac

service_id="com.github.csjaugustus.fih"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
tag="v$version"
dist="$repo/dist"
app="$dist/fih.app"
dmg="$dist/fih-$version.dmg"

identity="${FIH_SIGN_IDENTITY:-$(security find-identity -v -p codesigning |
    awk -F'"' '/Apple Development/ { print $2; exit }')}"
if [[ -z "$identity" ]]; then
    echo "warning: no signing certificate found; signing ad hoc, so users will" >&2
    echo "         have to grant Accessibility again after every update" >&2
    identity="-"
fi

if $publish; then
    # The release tag is made on GitHub, so what it points at must be there.
    if [[ -n "$(git status --porcelain)" ]]; then
        echo "error: commit your changes before publishing" >&2
        exit 1
    fi
    git fetch --quiet origin
    if ! git merge-base --is-ancestor HEAD "origin/$(git branch --show-current)"; then
        echo "error: push $(git branch --show-current) before publishing" >&2
        exit 1
    fi
    if gh release view "$tag" >/dev/null 2>&1; then
        echo "error: release $tag already exists; bump the version in Cargo.toml" >&2
        exit 1
    fi
fi

echo "==> building fih $version"
cargo build --release

echo "==> assembling $app"
rm -rf "$dist"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/fih "$app/Contents/MacOS/fih"
cp assets/fih.icns "$app/Contents/Resources/fih.icns"

# Opening the app runs fih as a launchd service from inside the bundle, which
# starts it at login and restarts it if it crashes. Opening it again just
# kicks the running service.
cat >"$app/Contents/MacOS/fih-launcher" <<'EOF'
#!/bin/sh
here="$(cd "$(dirname "$0")" && pwd)"
fih="$here/fih"
plist="$HOME/Library/LaunchAgents/com.github.csjaugustus.fih.plist"

# Run from the disk image, or from the read-only copy macOS makes of a
# downloaded app, the service would point at a path that later disappears.
case "$here" in
/Volumes/* | */AppTranslocation/*)
    osascript -e 'display alert "Move fih to Applications" message "Drag fih into the Applications folder, then open it from there."' >/dev/null
    exit 1
    ;;
esac

# Repoint a service installed from somewhere else (an older copy, or a build).
if [ -f "$plist" ] && ! grep -q "<string>$fih</string>" "$plist"; then
    "$fih" uninstall
fi
exec "$fih" start
EOF
chmod +x "$app/Contents/MacOS/fih-launcher"

cat >"$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleDisplayName</key>
    <string>fih</string>
    <key>CFBundleExecutable</key>
    <string>fih-launcher</string>
    <key>CFBundleIconFile</key>
    <string>fih</string>
    <key>CFBundleIdentifier</key>
    <string>$service_id.app</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>fih</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$version</string>
    <key>CFBundleVersion</key>
    <string>$version</string>
    <key>LSMinimumSystemVersion</key>
    <string>12.0</string>
    <key>LSUIElement</key>
    <true/>
  </dict>
</plist>
EOF

echo "==> signing"
# The daemon keeps the identifier the local deploy uses, so the Accessibility
# grant follows it from one version to the next.
codesign --force --sign "$identity" --identifier "$service_id" "$app/Contents/MacOS/fih"
codesign --force --sign "$identity" "$app"
codesign --verify --strict "$app"

echo "==> packaging $dmg"
staging="$dist/dmg"
mkdir -p "$staging"
cp -R "$app" "$staging/"
ln -s /Applications "$staging/Applications"
hdiutil create -quiet -volname "fih $version" -srcfolder "$staging" -ov -format UDZO "$dmg"
rm -rf "$staging"
codesign --force --sign "$identity" "$dmg"

if $publish; then
    echo "==> publishing $tag"
    gh release create "$tag" "$dmg" --target "$(git rev-parse HEAD)" \
        --title "fih $version" --notes-file - <<'EOF'
## Install

1. Open the DMG and drag **fih** into **Applications**.
2. Open fih from Applications. macOS blocks it the first time because it is not notarized: go to **System Settings → Privacy & Security**, scroll down and click **Open Anyway**.
3. Allow fih under **Accessibility** when asked. It needs this to move windows.

fih then lives in the menu bar and starts at login. To update, replace the app and open it again.

To remove it: run `/Applications/fih.app/Contents/MacOS/fih uninstall`, then move the app to the Bin.
EOF
fi

echo "==> done: $dmg"
