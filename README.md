# Agent Monitor —— 终端 AI 代理任务监控平台

监控多台电脑（Mac / Windows / Linux）上 Cursor / VSCode / 终端里正在执行的 AI 编码代理任务
（当前支持 **Claude Code**，进程识别已预留 **Codex**，会话解析可平行扩展），
并提供网页端查看与控制（暂停 / 恢复 / 中断 / 终止）。

```
┌─────────────────────────── agent-monitor ───────────────────────────┐
│                                                                      │
│  agent-monitor-web (React, 端口 3003)                                 │
│  ├── /portal   前台：Claude 式对话界面（左侧会话列表，右侧对话流+控制）      │
│  └── /admin    后管：仅用户管理（管理员 + 部署令牌双重锁）                  │
│                    │  /api → http://localhost:8383（webpack 代理）      │
│                    ▼                                                  │
│  agent-task-monitor (Rust axum, 端口 8383)                            │
│  ├── hub 模式（默认）：扫描本机 + 聚合多机 + HTTP/WS API + 登录契约        │
│  └── agent 模式（AM_HUB_URL=...）：扫描本机 → 定时上报 hub，执行下发命令   │
│         ▲                                    ▲                        │
│   Mac 电脑 A（hub 本机）                Windows 电脑 B（agent）           │
│   ~/.claude/projects/*.jsonl + 进程    %USERPROFILE%\.claude\...       │
└──────────────────────────────────────────────────────────────────────┘
```

## 工作原理

- **任务发现**：解析 `~/.claude/projects/**/*.jsonl` 会话文件（增量缓存），提取当前提示词、
  最近动作（正在调用的工具）、时间线；同时扫描系统进程找出 `claude`（及 `codex`）进程，
  沿父进程链识别宿主（Cursor / VSCode / Terminal/iTerm/Warp 等），按 cwd + 启动时间与会话配对。
- **状态判定**：`执行中`（回合未结束）/ `等待输入`（助手已答复）/ `已暂停`（SIGSTOP）/ `已结束`（进程退出）。
- **任务控制**：暂停 SIGSTOP、恢复 SIGCONT、中断 SIGINT、终止 SIGTERM、强杀 SIGKILL
  （Windows 走 taskkill，暂不支持暂停/恢复）。远程机器的控制命令经 hub 命令队列由 agent 拉取执行。
- **发布任务**：前台每个会话面板底部有对话框，可直接向正在运行的会话注入一行输入（回车发布）。
  macOS 优先用 AppleScript 按 tty 匹配 Terminal/iTerm 会话写入（只需一次"自动化"授权，无需 root）；
  匹配不到时回退 TIOCSTI（跨会话注入通常需以 root 运行监控端）。远程机器经命令队列由 agent 执行。
- **多机聚合**：agent 每 1.5s 向 hub `POST /monitor/report` 上报快照（活跃会话附带最近对话），
  10s 未上报判离线。任务/机器均带 `hostname`、`platform`（macos / windows / linux）标识。

## 应用程序形态（Mac / Windows）

`agent-task-monitor` 是一个**托盘应用**：启动后驻留 macOS 菜单栏 / Windows 系统托盘，
菜单提供「打开监控页面 / 打开后台管理 / 退出」；服务在后台运行。

- **macOS**：`bash scripts/package-macos.sh --with-web` 产出 `Agent Task Monitor.app`
  （内嵌前端构建产物，双击即用，不占 Dock）。
- **Windows**：`cargo build --release` 产出的 `agent-task-monitor.exe` 已隐藏控制台窗口，
  双击运行即出现托盘图标；把前端 `dist/` 放到 exe 旁的 `web/` 目录即可自带网页界面。
- 无 GUI 环境（服务器/CI）：`AM_NO_TRAY=1` 或用 `--no-default-features` 编译纯服务版。
- hub 检测到前端构建产物（`AM_WEB_DIST` > exe 旁 `web/` > `../agent-monitor-web/dist`）时
  会直接托管，浏览器访问 `http://localhost:8383/portal` 无需单独起前端。

## 终端与项目分组

- 识别的宿主：**Cursor / VSCode 内嵌终端**（父进程链优先判 IDE，Windows 下不会被
  powershell/cmd 误判），以及**独立终端**——mac 的 Terminal/iTerm/Warp/kitty 等、
  Windows 的 Windows Terminal / PowerShell / CMD、tmux。
- 前台左侧：**设备 → 终端类型（项目终端 = IDE 内嵌 / 外部终端）→ 会话** 三级可折叠树；
  会话项紧凑单行（状态点 + 名称 + 状态 + 拆分按钮）。
- 前台右侧支持**拆分视图**：像 IDE 终端拆分一样并排显示最多 4 个任务，各自独立控制；
  每格底部是 hsu-ui `Chat.Input` 对话框，可**直接发布任务/终端命令**（按 provider 提供
  `/clear`、`/compact`、`/model` 等快捷键）。
- 前台只显示**活跃会话**（有存活进程），退出后已结束的会话不再累积。

## 用户体系与信任设备（隐私）

- **前台需登录**，用户**只能看到自己名下、且已信任的设备**的会话；严格按归属隔离——
  超级管理员也看不到别人的设备会话。账号数据持久化到 `~/.agent-monitor/registry.json`。
- **信任设备**：agent 以 `AM_USER=<用户名>` 认领设备，首次接入登记为「待信任」；
  在前台「设备管理」抽屉里审批（信任 / 撤销 / 删除）。**未信任的设备 agent 不上报任何
  会话数据**（连有哪些终端都不外泄），hub 本机默认信任并归属超级管理员。
- **终端级排除**：在设备端托盘「监控范围」子菜单里勾选某个终端 = 不监控该终端，被排除
  的终端其会话不会被扫描/上报（持久化到 `~/.agent-monitor/excluded.json`）。
- **纯实时、不落存储**：任务快照与会话内容全程在内存、按需从 Claude 自己的 jsonl 实时
  读取，我方不持久化任何会话/对话内容；只持久化用户账号、设备信任、终端排除三项配置。

## 快速开始（单机）

```bash
# 1. 启动 Rust 监控端（hub 模式，端口 8383）
cd agent-task-monitor
cargo run --release

# 2. 启动前端（端口 3003，/api 已代理到 8383）
cd ../agent-monitor-web
yarn && yarn start
```

- 前台（需登录）：<http://localhost:3003/portal>，默认账号 `admin` / `admin123`，支持自助注册
- 后管（仅管理员）：<http://localhost:3003/admin>，登录后还需输入**后管访问令牌**解锁

## 安全机制

- **后管访问令牌**：hub 首次启动（部署）时生成 32 位随机令牌，输出到日志并持久化到
  `~/.agent-monitor/admin-token`（可用 `AM_ADMIN_TOKEN` 覆盖）。后管页面与全部
  `/sys/*` 管理接口要求「登录 + 超级管理员 + `X-Admin-Token` 头」三重校验；
  令牌校验失败有 800ms 延迟减缓暴力尝试。
- **后管仅保留用户管理**：查看/新增/改昵称/重置密码/删除注册用户；不记录任何日志类数据。
- **口令安全**：注册表中的口令以加盐迭代 SHA-256 存储（历史明文自动迁移）；
  登录、后管新增用户、重置密码的口令均经 RSA+AES 双层加密传输，不明文过网络。
- **危险输入防护（前台）**：向会话发布的内容命中危险模式（Claude Code 的
  `bypass permissions`、`rm -rf`、`sudo` 等）时，需两步确认（风险告知 + 输入确认词）；
  内置模式可在「设置 → 安全防护」中开关与自定义。
- **进程控制防越权**：控制/发布接口校验请求 pid 与任务快照 pid 一致，
  拒绝借任意 pid 操作无关进程。
- **CORS 默认同源**：hub 不再对任意来源开放跨域（`AM_CORS_ALLOW_ANY=1` 可放开）。

## 多机部署

在其它电脑（Mac / Windows）上编译并运行 agent 模式，指向 hub 机器的地址：

```bash
# macOS / Linux
AM_HUB_URL=http://<hub-ip>:8383 ./agent-task-monitor

# Windows (PowerShell)
$env:AM_HUB_URL="http://<hub-ip>:8383"; .\agent-task-monitor.exe
```

跨平台编译：`cargo build --release --target x86_64-pc-windows-msvc`（在对应平台构建最简单）。

## agent-task-monitor 环境变量

| 变量 | 默认 | 说明 |
| --- | --- | --- |
| `AM_PORT` | `8383` | hub 监听端口 |
| `AM_HUB_URL` | 无 | 设置后以 agent 模式运行，上报到该 hub |
| `AM_MACHINE_ID` | 主机名 | 机器唯一标识（多机重名时指定） |
| `AM_USERNAME` / `AM_PASSWORD` | `admin` / `admin123` | 超级管理员初始账号（首次启动种子） |
| `AM_ADMIN_TOKEN` | 首次启动随机生成 | 后管访问令牌（X-Admin-Token） |
| `AM_CORS_ALLOW_ANY` | 无 | 设为 `1` 时放开跨域（默认同源） |
| `AM_CRYPTO_KEY` | 内置开发密钥 | 登录 AES 密钥，需与前端 `.env` 的 `CRYPTO_KEY` 一致 |
| `AM_RSA_KEY_PATH` | 内置开发私钥 | RSA 私钥（PKCS#8 PEM），与前端 `RSA_PUB_KEY` 配对 |
| `AM_CLAUDE_PROJECTS_DIR` | `~/.claude/projects` | Claude Code 会话目录 |

> ⚠️ 内置密钥仅供本地开发。对外部署请用 `openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048`
> 重新生成密钥对，私钥走 `AM_RSA_KEY_PATH`，公钥（SPKI base64）填入前端 `.env` 的 `RSA_PUB_KEY`。

## HTTP API（hub，8383）

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/monitor/tasks?status=&keyword=` | 任务列表（前台，平铺参数） |
| GET | `/monitor/tasks/page?query=` | 任务分页（后管，vita-admin Query 格式） |
| GET | `/monitor/tasks/detail/:id` | 任务详情 |
| GET | `/monitor/tasks/:id/messages?limit=` | 会话对话流（远程机器读上报缓存） |
| POST | `/monitor/tasks/:id/control` | `{action: pause/resume/interrupt/stop/kill, pid?}` |
| POST | `/monitor/tasks/:id/input` | `{text, pid?}` —— 向会话发布一行输入（TTY 注入） |
| GET | `/monitor/machines` | 机器列表（在线状态 / 系统 / 会话数） |
| GET | `/monitor/agent` | hub 自身状态 |
| GET | `/monitor/ws` | WebSocket 实时任务快照 |
| POST | `/monitor/report` | agent 上报（内部协议） |
| GET/POST | `/auth/access/*`、`/sys/menu/*` | vita-admin 登录 / 菜单 / 权限契约 |

## 扩展新代理（如 Codex）

1. `src/process.rs` 的 `agent_kind()` 已按进程名识别 `codex`；
2. 会话解析：仿照 `src/scanner.rs`（Claude 来源）为 Codex 的会话存储格式实现一个 Scanner，
   在 `state.rs::local_scan` 中合并两路 `SessionSummary` 即可；
3. 前端无需改动（`provider` 字段驱动展示与筛选）。
