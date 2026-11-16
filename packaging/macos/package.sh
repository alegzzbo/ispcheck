#!/usr/bin/env bash
# Собирает «ISP Check.app» и ispcheck-macos-universal.dmg из готового бинаря.
# Использование: package.sh <бинарь> <версия> <папка-вывода>
# Запускать из корня репозитория (берёт favicon.ico для иконки, если он есть).
set -euo pipefail

BIN="$1"; VERSION="$2"; OUT="$3"

APP_NAME="ISP Check"             # имя приложения (в Finder и Launchpad)
BUNDLE_ID="com.example.ispcheck" # замените на свой идентификатор (обратная запись домена)
EXE="ispcheck"
DMG="$OUT/ispcheck-macos-universal.dmg"

APP="$OUT/$APP_NAME.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/$EXE"
chmod 755 "$APP/Contents/MacOS/$EXE"

# Иконка: favicon.ico -> AppIcon.icns (если файла нет, будет стандартная)
ICON_XML=""
if [ -f favicon.ico ]; then
  T="$(mktemp -d)"
  sips -s format png favicon.ico --out "$T/base.png" >/dev/null
  mkdir "$T/AppIcon.iconset"
  for s in 16 32 128 256 512; do
    sips -z "$s" "$s" "$T/base.png" --out "$T/AppIcon.iconset/icon_${s}x${s}.png" >/dev/null
    sips -z "$((s * 2))" "$((s * 2))" "$T/base.png" --out "$T/AppIcon.iconset/icon_${s}x${s}@2x.png" >/dev/null
  done
  iconutil -c icns "$T/AppIcon.iconset" -o "$APP/Contents/Resources/AppIcon.icns"
  ICON_XML="<key>CFBundleIconFile</key><string>AppIcon</string>"
fi

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>$APP_NAME</string>
  <key>CFBundleDisplayName</key><string>$APP_NAME</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleExecutable</key><string>$EXE</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
  $ICON_XML
</dict>
</plist>
PLIST

# Подпись «ad hoc» (без Apple Developer ID): нужна, чтобы бинарь arm64 вообще запускался.
# Gatekeeper всё равно спросит подтверждение при первом запуске (см. README).
codesign --force --deep --sign - "$APP"

# DMG: приложение + ярлык на /Applications для перетаскивания
STAGE="$(mktemp -d)"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
rm -f "$DMG"
hdiutil create -volname "$APP_NAME" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
echo "$DMG"
