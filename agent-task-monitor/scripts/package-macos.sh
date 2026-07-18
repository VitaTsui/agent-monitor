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

echo "▸ cargo build --release --features desktop（内置默认 hub: ${HUB_URL}）"
AM_DEFAULT_HUB_URL="${HUB_URL}" cargo build --release --features desktop

rm -rf "$OUT"
mkdir -p "$OUT/Contents/MacOS" "$OUT/Contents/Resources"
cp target/release/agent-task-monitor "$OUT/Contents/MacOS/$EXE_NAME"

if [[ -f icons/icon.icns ]]; then
  cp icons/icon.icns "$OUT/Contents/Resources/icon.icns"
else
  echo "  ! 缺 icons/icon.icns，图标会退回系统默认"
fi

# 客户端配置（可选覆盖）：正常使用零配置 —— hub 地址已编译内置，
# 首次打开在窗口里登录账号即自动绑定本机。此文件仅供高级覆盖。
cat > "$OUT/Contents/Resources/config.txt" <<CONF
# 终端任务监控 · 可选配置（正常使用无需修改任何内容）
# 在客户端窗口里登录账号即可完成绑定并开始同步。
# 高级覆盖示例：
#   AM_HUB_URL=https://your-own-hub.example.com   # 自建 hub 时指向自己的服务
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
  <key>CFBundleVersion</key><string>0.2.0</string>
  <key>CFBundleShortVersionString</key><string>0.2.0</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <!-- 不设 LSUIElement：这是正常桌面应用（Dock 有图标、可 Cmd-Tab）。
       「最小化到托盘 / agent 后台模式」由运行时 ActivationPolicy::Accessory 切换，
       plist 写死 UIElement 会把正常模式也压成无 Dock 图标。 -->
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

# ad-hoc 深度签名：不签的话链接器签名只覆盖二进制，bundle 里的 icon/config
# 不在签名内 —— 用户从网上下载（带隔离标记）后 macOS 会直接判「已损坏」，
# 连右键打开都无法绕过。ad-hoc 签完变成「无法验证开发者」，右键→打开即可运行。
codesign --force --deep -s - "$OUT"
codesign --verify --deep --strict "$OUT" && echo "▸ ad-hoc 签名校验通过"

echo "✓ 打包完成: $OUT"
echo "  零配置分发：用户安装后在窗口里登录账号即自动绑定本机"
