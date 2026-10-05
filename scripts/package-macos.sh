#!/usr/bin/env bash
# Build RetroGit.app from a compiled binary, sign it, and zip it.
#
#   scripts/package-macos.sh <binary> <version> <output dir>
#
# Signed ad-hoc ("-") unless $MACOS_SIGN_IDENTITY names another identity (a Developer ID one
# day). The zip keeps the signature (ditto). Notifications need the app to be signed.
set -euo pipefail
bin="$1"; version="$2"; out="$3"
root="$(cd "$(dirname "$0")/.." && pwd)"
app="$out/RetroGit.app"
rm -rf "$app"
mkdir -p "$out" "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$bin" "$app/Contents/MacOS/retrogit"
icns="$root/crates/app/assets/RetroGit.icns"
if [ ! -f "$icns" ]; then
  echo "missing $icns: the app would have no icon" >&2
  exit 1
fi
cp "$icns" "$app/Contents/Resources/RetroGit.icns"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>io.github.madjidsahki.retrogit</string>
  <key>CFBundleName</key><string>RetroGit</string>
  <key>CFBundleDisplayName</key><string>RetroGit</string>
  <key>CFBundleExecutable</key><string>retrogit</string>
  <key>CFBundleIconFile</key><string>RetroGit</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSHumanReadableCopyright</key><string>RetroGit</string>
</dict>
</plist>
PLIST
identity="${MACOS_SIGN_IDENTITY:--}"
# Extended attributes (provenance, quarantine) would end up as ._ files in the zip.
xattr -cr "$app"
echo "Signing with: $identity"
# A real identity gets Apple's secure timestamp (notarization needs it); ad-hoc cannot.
if [ "$identity" = "-" ]; then timestamp="--timestamp=none"; else timestamp="--timestamp"; fi
codesign --force --options runtime "$timestamp" --sign "$identity" "$app"
codesign --verify --deep --strict "$app"
# The bundle says which version it is (checked by the CI).
test "$(/usr/libexec/PlistBuddy -c 'Print CFBundleVersion' "$app/Contents/Info.plist")" = "$version"
zip="$out/RetroGit-macos-arm64.zip"
rm -f "$zip"
# No resource forks, extended attributes or quarantine: they would become ._ entries.
(cd "$out" && ditto -c -k --norsrc --noextattr --noqtn --keepParent RetroGit.app RetroGit-macos-arm64.zip)
# Listed first (a broken zip stops here), then searched: no early-exiting pipe under pipefail.
listing="$(unzip -Z1 "$zip")"
if grep -Eq '(^|/)(\._|__MACOSX)' <<<"$listing"; then
  echo "$zip contains ._ or __MACOSX entries" >&2
  exit 1
fi
echo "Wrote $app and $zip"
