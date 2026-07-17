#!/usr/bin/env bash
# 打包 macOS 托盘客户端：终端任务监控.app
#
# 产出与 clients/mac-app/ 下实际分发的一致：托盘常驻、不占 Dock、
# 自带 config.txt 指向云端 hub（即 agent 模式，把本机会话上报上去）。
#
# 用法:
#   bash scripts/package-macos.sh                      # config.txt 里留占位令牌
#   AM_AGENT_TOKEN=xxx bash scripts/package-macos.sh   # 直接写入真实上报令牌
#
# 两个容易踩空的点：
#   - 必须带 --features desktop，否则打出来的只是个命令行服务，没有托盘也没有窗口；
#   - 必须放 config.txt，否则读不到 AM_HUB_URL，客户端会当自己是 hub、在本机 8383
#     起服务，而不是上报云端 —— 用户看到的现象是「装了但网页上一直没有这台设备」。
set -euo pipefail

cd "$(dirname "$0")/.."

APP_NAME="终端任务监控"
BUNDLE_ID="com.vitahsu.agentmonitor"
# 可执行文件名必须与 Info.plist 的 CFBundleExecutable 一致
EXE_NAME="agent-monitor"
HUB_URL="${AM_HUB_URL:-https://monitor.vita-llm.com}"
OUT="target/release/bundle/$APP_NAME.app"

echo "▸ cargo build --release --features desktop"
cargo build --release --features desktop

rm -rf "$OUT"
mkdir -p "$OUT/Contents/MacOS" "$OUT/Contents/Resources"
cp target/release/agent-task-monitor "$OUT/Contents/MacOS/$EXE_NAME"

if [[ -f icons/icon.icns ]]; then
  cp icons/icon.icns "$OUT/Contents/Resources/icon.icns"
else
  echo "  ! 缺 icons/icon.icns，图标会退回系统默认"
fi

# 客户端配置：程序按 <可执行文件>/../Resources/config.txt 查找（见 main.rs）
cat > "$OUT/Contents/Resources/config.txt" <<CONF
# 终端任务监控 · 客户端配置
# 把 AM_USER 改成你在网站上的登录用户名，保存即可。
AM_USER=admin
AM_HUB_URL=$HUB_URL
AM_AGENT_TOKEN=${AM_AGENT_TOKEN:-请向管理员索取上报令牌}
CONF

cat > "$OUT/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>$APP_NAME</string>
  <key>CFBundleDisplayName</key><string>$APP_NAME</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleExecutable</key><string>$EXE_NAME</string>
  <key>CFBundleIconFile</key><string>icon.icns</string>
  <key>CFBundleVersion</key><string>0.1.0</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <!-- 只驻留菜单栏、不占 Dock。注意 Tauri 建窗口时会覆盖它，
       代码里还显式设了 ActivationPolicy::Accessory，两处都需要。 -->
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

echo "✓ 打包完成: $OUT"
echo "  分发前把 config.txt 的 AM_AGENT_TOKEN 换成 hub 上的真实令牌（~/.agent-monitor/agent-token）"
