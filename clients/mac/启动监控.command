#!/usr/bin/env bash
# 双击运行：把本机 Claude Code 会话上报到云端监控。
# 首次运行若被 macOS 拦截：右键点本文件 → 打开，或到「系统设置 → 隐私与安全性」放行。
cd "$(dirname "$0")"

# ↓↓↓ 改成你在网站上的登录用户名 ↓↓↓
AM_USER="admin"

export AM_HUB_URL="https://monitor.vita-llm.com"
export AM_AGENT_TOKEN="***REMOVED***"
export AM_USER

echo "正在连接 https://monitor.vita-llm.com （用户: $AM_USER）..."
echo "保持本窗口开启即持续上报；关闭窗口即停止。"
echo "首次接入后，去网站『设备管理』里信任本设备，会话才会显示。"
echo
xattr -dr com.apple.quarantine ./agent-task-monitor 2>/dev/null || true
exec ./agent-task-monitor
