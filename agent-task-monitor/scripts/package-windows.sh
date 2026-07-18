#!/usr/bin/env bash
# 打包 Windows 客户端安装程序（macOS/Linux 交叉构建）：
#   cargo-xwin 编译 GUI 程序 → NSIS 生成中文安装向导（可选安装位置 / 桌面图标 / 开机自启）。
#
# 依赖：cargo-xwin（cargo install cargo-xwin）、makensis（brew install makensis）
# 用法：bash scripts/package-windows.sh   # 产出 target/dist/终端任务监控.exe（安装程序）
set -euo pipefail
cd "$(dirname "$0")/.."

HUB_URL="${AM_HUB_URL:-https://monitor.vita-llm.com}"
OUT_DIR="target/dist"
mkdir -p "$OUT_DIR"

echo "▸ cargo xwin build -p am-client --release（内置默认 hub: ${HUB_URL}）"
AM_DEFAULT_HUB_URL="${HUB_URL}" cargo xwin build -p am-client --release \
  --target x86_64-pc-windows-msvc

echo "▸ makensis 生成安装向导"
makensis \
  -DEXE="$PWD/target/x86_64-pc-windows-msvc/release/agent-monitor.exe" \
  -DICO="$PWD/client/icons/icon.ico" \
  "-DOUT=$PWD/$OUT_DIR/终端任务监控.exe" \
  scripts/windows-installer.nsi >/dev/null

echo "✓ 安装程序: $OUT_DIR/终端任务监控.exe"
echo "  向导语言：简体中文；可选安装位置、桌面快捷方式、开机自启；含卸载器"
