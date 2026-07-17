@echo off
chcp 65001 >nul
taskkill /IM 终端任务监控.exe /F >nul 2>&1
reg delete "HKCU\Software\Microsoft\Windows\CurrentVersion\Run" /v AgentMonitor /f >nul 2>&1
del "%APPDATA%\Microsoft\Windows\Start Menu\Programs\终端任务监控.lnk" >nul 2>&1
rmdir /S /Q "%LOCALAPPDATA%\AgentMonitor" >nul 2>&1
echo 已卸载（含开机自启项与开始菜单快捷方式）。
pause
