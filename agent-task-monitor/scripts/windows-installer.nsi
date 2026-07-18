; 终端任务监控 · Windows 安装向导（NSIS / MUI2，简体中文）
; 用法（macOS/Linux 交叉打包）：
;   makensis -DEXE=target/x86_64-pc-windows-msvc/release/agent-monitor.exe \
;            -DICO=client/icons/icon.ico -DOUT=终端任务监控.exe \
;            scripts/windows-installer.nsi
; 产出即官网分发的安装程序：向导可选安装位置、是否创建桌面图标、是否开机自启。
Unicode true
ManifestDPIAware true

!define APP_NAME "终端任务监控"
; 安装路径全英文：目录/可执行文件/卸载器均无中文，避免个别环境的编码问题；
; 中文只用于「显示名」（快捷方式、开始菜单、卸载列表）。
!define APP_EXE "AgentMonitor.exe"
!define APP_EXE_LEGACY "终端任务监控.exe"
!define APP_ID "AgentMonitor"
!define APP_PUBLISHER "VitaHsu"
!define APP_VERSION "0.3.14"
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
InstallDir "$LOCALAPPDATA\${APP_ID}"
InstallDirRegKey HKCU "Software\${APP_ID}" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma

; 文件属性里的版本信息（资源管理器「详细信息」/ SmartScreen 展示用）
VIProductVersion "${APP_VERSION}.0"
VIAddVersionKey /LANG=2052 "ProductName" "Agent Monitor"
VIAddVersionKey /LANG=2052 "FileDescription" "${APP_NAME}安装程序"
VIAddVersionKey /LANG=2052 "ProductVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=2052 "FileVersion" "${APP_VERSION}"
VIAddVersionKey /LANG=2052 "CompanyName" "${APP_PUBLISHER}"
VIAddVersionKey /LANG=2052 "LegalCopyright" "© ${APP_PUBLISHER}"

!include "MUI2.nsh"
!define MUI_ICON "${ICO}"
!define MUI_UNICON "${ICO}"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP_EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "立即启动 ${APP_NAME}"
; 启动时把向导窗口带到前台：部分环境（从浏览器下载栏直接运行）下
; 窗口会启动在后台，用户误以为「点了没反应」，得去点任务栏图标才出来。
; .onGUIInit 时机太早（窗口尚未显示，BringToFront 落空），
; 必须挂在首页 SHOW 回调上，并直接调 SetForegroundWindow 双保险。
!define MUI_CUSTOMFUNCTION_GUIINIT BringInstallerToFront
!define MUI_PAGE_CUSTOMFUNCTION_SHOW WelcomeShow
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"

Function BringInstallerToFront
  BringToFront
FunctionEnd

Function WelcomeShow
  ; 窗口已可见的时点：TOPMOST 闪置顶（不受前台锁限制，强制到最上层）
  ; 再取消 TOPMOST 并请求前台焦点
  System::Call "user32::SetWindowPos(p $HWNDPARENT, p -1, i 0, i 0, i 0, i 0, i 3)"
  System::Call "user32::SetWindowPos(p $HWNDPARENT, p -2, i 0, i 0, i 0, i 0, i 3)"
  System::Call "user32::SetForegroundWindow(p $HWNDPARENT)"
  BringToFront
FunctionEnd

Function .onInit
  ; 旧版本默认装在中文目录：升级时迁移到英文目录（用户自选过其它目录则尊重）
  StrCmp $INSTDIR "$LOCALAPPDATA\${APP_NAME}" 0 +2
    StrCpy $INSTDIR "$LOCALAPPDATA\${APP_ID}"
FunctionEnd

Section "主程序（必装）" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"
  ; 覆盖安装前先结束运行中的旧实例（含旧中文名），避免文件占用
  nsExec::Exec 'taskkill /F /IM "${APP_EXE}"'
  nsExec::Exec 'taskkill /F /IM "${APP_EXE_LEGACY}"'
  File "/oname=${APP_EXE}" "${EXE}"
  ; 过渡兼容：旧版客户端的静默更新脚本按旧中文名重启，保留一个同内容副本；
  ; 旧「开机自启」注册表项指向旧名时也能继续工作。后续版本可移除。
  CopyFiles /SILENT "$INSTDIR\${APP_EXE}" "$INSTDIR\${APP_EXE_LEGACY}"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  WriteRegStr HKCU "Software\${APP_ID}" "InstallDir" "$INSTDIR"
  ; 旧版遗留清理：中文目录里的旧程序与卸载器（迁移到英文目录后不再使用）
  Delete "$LOCALAPPDATA\${APP_NAME}\${APP_EXE_LEGACY}"
  Delete "$LOCALAPPDATA\${APP_NAME}\卸载.exe"
  RMDir "$LOCALAPPDATA\${APP_NAME}"
  ; 开机自启项若已存在，改指向新路径（旧路径的程序已被清理）
  ReadRegStr $0 HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${APP_ID}"
  StrCmp $0 "" +2
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${APP_ID}" '"$INSTDIR\${APP_EXE}" --background'
  ; 开始菜单
  CreateDirectory "$SMPROGRAMS\${APP_NAME}"
  CreateShortCut "$SMPROGRAMS\${APP_NAME}\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}"
  CreateShortCut "$SMPROGRAMS\${APP_NAME}\卸载 ${APP_NAME}.lnk" "$INSTDIR\Uninstall.exe"
  ; 「设置 → 应用」卸载入口
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\${APP_EXE}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "${APP_PUBLISHER}"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
SectionEnd

Section "桌面快捷方式" SecDesktop
  CreateShortCut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}"
SectionEnd

Section /o "开机自动启动" SecAutostart
  ; 与客户端内「开机自启」开关同一注册表项；--background = 开机静默进托盘
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "${APP_ID}" '"$INSTDIR\${APP_EXE}" --background'
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMain} "程序本体与开始菜单项（必装）。"
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "在桌面创建「${APP_NAME}」图标。"
  !insertmacro MUI_DESCRIPTION_TEXT ${SecAutostart} "随 Windows 启动，在后台持续同步本机终端会话。"
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "Uninstall"
  nsExec::Exec 'taskkill /F /IM "${APP_EXE}"'
  nsExec::Exec 'taskkill /F /IM "${APP_EXE_LEGACY}"'
  Delete "$INSTDIR\${APP_EXE}"
  Delete "$INSTDIR\${APP_EXE_LEGACY}"
  Delete "$INSTDIR\Uninstall.exe"
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
