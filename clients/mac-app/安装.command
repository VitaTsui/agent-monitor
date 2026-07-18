#!/bin/bash
# 双击安装：把「终端任务监控」装进「应用程序」、去除下载隔离并启动。
# 若双击被拦（未验证开发者）：右键本文件 →「打开」。
cd "$(dirname "$0")"
APP="终端任务监控.app"
DEST="/Applications/$APP"
echo "正在安装到 /Applications ..."
# 覆盖安装前先退出运行中的旧实例，避免装完出现两个实例同时上报
pkill -f "$APP/Contents/MacOS/agent-monitor" 2>/dev/null && sleep 1
rm -rf "$DEST" 2>/dev/null
cp -R "$APP" "$DEST"
xattr -dr com.apple.quarantine "$DEST" 2>/dev/null
echo "✓ 已安装"
open "$DEST"
echo "已启动。首次使用：在打开的窗口里登录你的账号，本机即自动绑定。"
echo "（图标在右上角菜单栏；右键可设「开机自启」）"
