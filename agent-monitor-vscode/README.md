# 终端任务监控 · 桥接扩展（Cursor / VSCode）

让「终端任务监控」桌面客户端能向 **Cursor / VSCode 的内嵌终端** 下发任务。

## 为什么需要它

内嵌终端走 ConPTY（伪控制台），桌面客户端无法用 `WriteConsoleInput` 往里注入。本扩展在
编辑器里用官方 API `terminal.sendText()` 直接把任务写进对应终端并回车执行。

客户端与扩展通过 `<数据目录>/bridge/` 下的文件通信（无需网络/端口）：
- 扩展每 2s 写 `win-<pid>.json` 心跳（含本窗口各终端的 shell pid）；
- 客户端要对某内嵌终端下发时，写 `outbox/<ts>-<pid>.json`；
- 扩展轮询 outbox，pid 命中本窗口的终端就 `sendText` 送达。

数据目录与客户端一致：Windows `%APPDATA%\AgentMonitor`、mac `~/Library/Application Support/AgentMonitor`、
Linux `~/.local/share/AgentMonitor`（或 `AM_DATA_DIR`）。

## 构建 .vsix

```bash
cd agent-monitor-vscode
npm install
npm run package     # 生成 agent-monitor-bridge.vsix
```

## 安装

- 命令行：`code --install-extension agent-monitor-bridge.vsix`
  （Cursor 用 `cursor --install-extension agent-monitor-bridge.vsix`）
- 或在编辑器里：扩展面板 → 右上 `...` → 「从 VSIX 安装」。

安装后重载窗口即生效；无需登录，只在本机文件目录内工作。可用命令面板运行
「终端任务监控：桥接状态」查看当前监听的终端数。
