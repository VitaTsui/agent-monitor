@echo off
chcp 65001 >nul
REM 双击运行：把本机 Claude Code 会话上报到云端监控。

REM ↓↓↓ 改成你在网站上的登录用户名 ↓↓↓
set AM_USER=admin

set AM_HUB_URL=https://monitor.vita-llm.com
set AM_AGENT_TOKEN=***REMOVED***

echo 正在连接 https://monitor.vita-llm.com  用户: %AM_USER%
echo 保持本窗口开启即持续上报；关闭窗口即停止。
echo 首次接入后，去网站『设备管理』里信任本设备，会话才会显示。
echo.
"%~dp0agent-task-monitor.exe"
pause
