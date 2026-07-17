@echo off
chcp 65001 >nul
setlocal
set DST=%LOCALAPPDATA%\AgentMonitor
echo 正在安装到 %DST% ...
if not exist "%DST%" mkdir "%DST%"
copy /Y "%~dp0终端任务监控.exe" "%DST%\终端任务监控.exe" >nul
copy /Y "%~dp0config.txt" "%DST%\config.txt" >nul

REM 开始菜单快捷方式
set SM=%APPDATA%\Microsoft\Windows\Start Menu\Programs
powershell -NoProfile -Command ^
  "$w=New-Object -ComObject WScript.Shell; $s=$w.CreateShortcut('%SM%\终端任务监控.lnk'); $s.TargetPath='%DST%\终端任务监控.exe'; $s.WorkingDirectory='%DST%'; $s.Save()"

echo.
echo 安装完成。已复制到 %DST% 并创建开始菜单快捷方式。
echo 提示：用记事本打开 %DST%\config.txt 把 AM_USER 改成你的登录用户名。
echo 现在启动一次…
start "" "%DST%\终端任务监控.exe"
echo 程序已在后台运行（系统托盘查看图标）。右键托盘图标可开启「开机自启」。
pause
