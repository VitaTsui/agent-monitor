#!/bin/bash
# 双击：把「终端任务监控.app」装进「应用程序」并去除隔离，然后启动。
cd "$(dirname "$0")"
APP="终端任务监控.app"
DEST="/Applications/$APP"
echo "正在安装到 /Applications ..."
rm -rf "$DEST" 2>/dev/null
cp -R "$APP" "$DEST"
xattr -dr com.apple.quarantine "$DEST" 2>/dev/null
echo "已安装到 $DEST"
echo "提示：如需改登录用户名，右键 $APP →「显示包内容」→ Contents/Resources/config.txt"
open "$DEST"
echo "已启动，图标在屏幕右上角菜单栏。右键图标可开启「开机自启」。"
