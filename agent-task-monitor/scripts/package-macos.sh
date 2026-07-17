#!/usr/bin/env bash
# 打包 macOS 应用：Agent Task Monitor.app
# 用法: bash scripts/package-macos.sh [--with-web]
#   --with-web  先构建前端并打进 .app（Contents/Resources/web），
#               应用自带完整网页界面，浏览器访问 http://localhost:8383
set -euo pipefail

cd "$(dirname "$0")/.."
APP_NAME="Agent Task Monitor"
BUNDLE_ID="com.vitahsu.agent-task-monitor"
OUT="target/release/bundle/$APP_NAME.app"

echo "▸ cargo build --release"
cargo build --release

if [[ "${1:-}" == "--with-web" ]]; then
  echo "▸ 构建前端 (agent-monitor-web)"
  (cd ../agent-monitor-web && yarn build)
fi

rm -rf "$OUT"
mkdir -p "$OUT/Contents/MacOS" "$OUT/Contents/Resources"
cp target/release/agent-task-monitor "$OUT/Contents/MacOS/"

if [[ -d ../agent-monitor-web/dist ]]; then
  cp -R ../agent-monitor-web/dist "$OUT/Contents/Resources/web"
  echo "▸ 已内嵌前端静态资源"
fi

cat > "$OUT/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>$APP_NAME</string>
  <key>CFBundleDisplayName</key><string>$APP_NAME</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleVersion</key><string>0.1.0</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleExecutable</key><string>agent-task-monitor</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

echo "✓ 打包完成: $OUT"
echo "  双击启动后驻留菜单栏；LSUIElement=true 不占 Dock。"
