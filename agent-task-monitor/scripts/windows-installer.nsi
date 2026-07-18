; 终端任务监控 · Windows 安装向导（NSIS / MUI2，简体中文）
; 用法（macOS/Linux 交叉打包）：
;   makensis -DEXE=target/x86_64-pc-windows-msvc/release/agent-monitor.exe \
;            -DICO=client/icons/icon.ico -DOUT=终端任务监控.exe \
;            scripts/windows-installer.nsi
; 产出即官网分发的安装程序：向导可选安装位置、是否创建桌面图标、是否开机自启。
Unicode true
ManifestDPIAware true

!define APP_NAME "终端任务监控"
!define APP_EXE "终端任务监控.exe"
!define APP_ID "AgentMonitor"
!define APP_PUBLISHER "VitaHsu"
!define APP_VERSION "0.3.2"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP_ID}"

!ifndef EXE
  !error "请以 -DEXE=<agent-monitor.exe 路径> 调用"
!endif
!ifndef OUT
  !define OUT "终端任务监控-安装程序.exe"
!endif

Name "${APP_NAME}"
OutFile "${OUT}"
; 用户级安装（无需管理员），默认装到本地应用目录；向导页可改
InstallDir "$LOCALAPPDATA\${APP_NAME}"
InstallDirRegKey HKCU "Software\${APP_ID}" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma

!include "MUI2.nsh"
!define MUI_ICON "${ICO}"
!define MUI_UNICON "${ICO}"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP_EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "立即启动 ${APP_NAME}"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"

Section "主程序（必装）" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"
  ; 覆盖安装前先结束运行中的旧实例，避免文件占用
  nsExec::Exec 'taskkill /F /IM "${APP_EXE}"'
  File "/oname=${APP_EXE}" "${EXE}"
  WriteUninstaller "$INSTDIR\卸载.exe"
  WriteRegStr HKCU "Software\${APP_ID}" "InstallDir" "$INSTDIR"
  ; 开始菜单
  CreateDirectory "$SMPROGRAMS\${APP_NAME}"
  CreateShortCut "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}"
  CreateShortCut "$SMPROGRAMS\${APP_NAME}\卸载 ${APP_NAME}.lnk" "$INSTDIR\卸载.exe"
  ; 「设置 → 应用」卸载入口
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\${APP_EXE}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "${APP_PUBLISHER}"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\卸载.exe"'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
SectionEnd

Section "桌面快捷方式" SecDesktop
  CreateShortCut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}"
SectionEnd

Section /o "开机自动启动" SecAutostart
  ; 与客户端内「开机自启」开关同一注册表项，装完后也可随时在应用里改
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${APP_ID}" '"$INSTDIR\${APP_EXE}"'
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMain} "程序本体与开始菜单项（必装）。"
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "在桌面创建「${APP_NAME}」图标。"
  !insertmacro MUI_DESCRIPTION_TEXT ${SecAutostart} "随 Windows 启动，在后台持续同步本机终端会话。"
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "Uninstall"
  nsExec::Exec 'taskkill /F /IM "${APP_EXE}"'
  Delete "$INSTDIR\${APP_EXE}"
  Delete "$INSTDIR\卸载.exe"
  RMDir "$INSTDIR"
  Delete "$DESKTOP\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk"
  Delete "$SMPROGRAMS\${APP_NAME}\卸载 ${APP_NAME}.lnk"
  RMDir "$SMPROGRAMS\${APP_NAME}"
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${APP_ID}"
  DeleteRegKey HKCU "${UNINST_KEY}"
  DeleteRegKey HKCU "Software\${APP_ID}"
  ; 数据目录（%USERPROFILE%\.agent-monitor）保留：里面有设备绑定令牌，
  ; 重装后免重新配对；用户想彻底清除可手动删除该目录。
SectionEnd
