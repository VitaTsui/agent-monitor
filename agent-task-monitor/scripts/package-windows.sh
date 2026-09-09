#!/usr/bin/env bash
# 打包 Windows 客户端安装程序（macOS/Linux 交叉构建，或直接在 Windows 上跑）：
#   编译 GUI 程序 → NSIS 生成中文安装向导（可选安装位置 / 桌面图标 / 开机自启）。
#
# 依赖：makensis（brew install makensis）；在 macOS/Linux 上另需 cargo-xwin（cargo install cargo-xwin）
# 用法：bash scripts/package-windows.sh   # 产出 target/dist/终端任务监控.exe（安装程序）
set -euo pipefail
cd "$(dirname "$0")/.."

HUB_URL="${AM_HUB_URL:-https://monitor.vita-llm.com}"
OUT_DIR="target/dist"
VERSION=$(grep -m1 '^version = ' Cargo.toml | sed 's/version = "\(.*\)"/\1/')
mkdir -p "$OUT_DIR"

# 在 Windows 上跑（CI 的 windows runner、或本机就是 Windows）时没有「交叉」可言：
# cargo-xwin 是给 macOS/Linux 拉 MSVC SDK 用的，本地已有 MSVC 时直接 cargo build。
if [[ "$(uname -s)" == MINGW* || "$(uname -s)" == MSYS* || "$(uname -s)" == CYGWIN* ]]; then
  echo "▸ cargo build -p am-client --release（Windows 本机，内置默认 hub: ${HUB_URL}）"
  AM_DEFAULT_HUB_URL="${HUB_URL}" cargo build -p am-client --release \
    --target x86_64-pc-windows-msvc
else
  echo "▸ cargo xwin build -p am-client --release（交叉编译，内置默认 hub: ${HUB_URL}）"
  AM_DEFAULT_HUB_URL="${HUB_URL}" cargo xwin build -p am-client --release \
    --target x86_64-pc-windows-msvc
fi

# makensis 要绝对路径：NSIS 里的相对路径是相对 .nsi 所在目录（scripts/）算的。
# 而在 Git Bash 里 $PWD 是 /d/a/... —— 原生的 makensis.exe 读不懂，
# 得用 cygpath 转成 D:\a\...；非 Windows 上没有 cygpath，原样返回。
if command -v cygpath >/dev/null 2>&1; then
  winpath() { cygpath -w "$1"; }
else
  winpath() { printf '%s' "$1"; }
fi

echo "▸ makensis 生成安装向导"
makensis \
  -DEXE="$(winpath "$PWD/target/x86_64-pc-windows-msvc/release/agent-monitor.exe")" \
  -DICO="$(winpath "$PWD/client/icons/icon.ico")" \
  -DAPP_VERSION="${VERSION}" \
  "-DOUT=$(winpath "$PWD/$OUT_DIR/AgentMonitor-${VERSION}-setup.exe")" \
  scripts/windows-installer.nsi >/dev/null

echo "✓ 安装程序: $OUT_DIR/AgentMonitor-${VERSION}-setup.exe"
echo "  向导语言：简体中文；可选安装位置、桌面快捷方式、开机自启；含卸载器"
