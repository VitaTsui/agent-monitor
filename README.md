# Agent Monitor —— 终端 AI 代理任务监控平台

[![license](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)
[![rust](https://img.shields.io/badge/rust-1.77%2B-CE422B.svg)](./agent-task-monitor/Cargo.toml)
[![react](https://img.shields.io/badge/react-18-61DAFB.svg)](./agent-monitor-web/package.json)

监控多台电脑（Mac / Windows / Linux）上 Cursor / VSCode / 终端里正在执行的 AI 编码代理任务
（当前支持 **Claude Code**，进程识别已预留 **Codex**，会话解析可平行扩展），
并提供网页端查看与控制（暂停 / 恢复 / 中断 / 终止）。

```
┌─────────────────────────── agent-monitor ───────────────────────────┐
│                                                                      │
│  agent-monitor-web (React, 端口 3003)                                 │
│  ├── /portal   前台：Claude 式对话界面（左侧会话列表，右侧对话流+控制）      │
│  └── /admin    后管：用户管理 / 版本管理 / 机器人接入（登录 + 超级管理员）   │
│                    │  /api → http://localhost:8383（Vite 代理）        │
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
  会话默认回溯 **30 天**（`AM_HISTORY_DAYS` 可调）。
- **子任务（子代理 / 后台命令）**：会话对象上带 `subTasks[]`，每条给出 `outcome`
  ——`running` / `completed` / `failed` / `interrupted`。归类看的是磁盘事实（子会话
  `subagents/agent-<id>.jsonl` 有没有把结果交回去），不是上游文案；`interrupted` 表示
  被父会话退出连带终止，**不是失败**，前端配色与判断一律只看 `outcome`
  （`status` 是上游原文，另有后端合成的 `orphaned`，仅供排障追溯）。
- **终态是吸收态，重新派活用 `runs` 标出来**：收尾判定有两条来源 —— 父记录写下的收尾
  通知，以及父记录没写时从磁盘推断（尾形态已收尾 + 静置超过 5 分钟）。磁盘推断出来的
  终态**一旦落定就记住**，「文件又被写了一下」不算证据；只有父记录发话才能重开
  （新的一次派活 = 新的 `toolUseId`、新的收尾通知、或 `TaskStop`）。
  同一个子代理可能被反复叫起来干活，而**重新派活不写新的调用记录**（后端只能就地改写
  旧记录的 `endedMs`），所以 `SubTask.runs`（第几次派活，从 1 起）才是唯一的判据：
  **`runs` 变了 = 它真的又跑起来了；`runs` 没变却从终态翻回 `running` = 后端有 bug**。
  实测真实会话按行增量重放 599 次采样，回抖 0 次。
  代价（是行为约定不是缺陷）：子代理若真卡在「已交回结果」的形态超过 5 分钟又继续跑，
  会一直显示 `completed`，直到它的收尾通知到达才纠正；这份「已落定」不跨进程持久化，
  客户端重启后按同样规则重推一次（结果相同，只在重启那刻文件刚好 5 分钟内被写过时多一次
  `running → completed` 的单向过渡，不会来回抖）。
- **工具调用是结构化的**：`role:"tool"` 的消息把每次调用单列一个元素放进
  `tools: ToolCall[]`（`ToolCall = {id?, name, hint?}`，`id` 即 `tool_use_id`，Codex 那边
  是 `call_id`），此时 `content` 是空串；`role:"tool_result"` 的消息带 `toolUseId` 指回
  它回应的那次调用。`SubTask` 也带 `toolUseId` = 起跑它的那次调用 —— 前端靠
  `tool.id === subTask.toolUseId` **精确配对**，配不上就按普通工具调用画，不做文字匹配。
  拿不到 id 时字段整个不下发（不是空串也不是 null）。
- **状态判定**：`执行中`（回合未结束）/ `等待输入`（助手已答复）/ `已暂停`（SIGSTOP）/ `已结束`（进程退出）。
- **任务控制**：暂停 SIGSTOP、恢复 SIGCONT、中断 SIGINT、终止 SIGTERM、强杀 SIGKILL
  （Windows 走 taskkill，暂不支持暂停/恢复）。远程机器的控制命令经 hub 命令队列由 agent 拉取执行。
- **发布任务**：前台每个会话面板底部有对话框，可直接向正在运行的会话注入一行输入（回车发布）。
  macOS 优先用 AppleScript 按 tty 匹配 Terminal/iTerm 会话写入（只需一次"自动化"授权，无需 root）；
  匹配不到时回退 TIOCSTI（跨会话注入通常需以 root 运行监控端）。远程机器经命令队列由 agent 执行。
- **多机聚合**：agent 向 hub `POST /monitor/report` 上报快照，10s 未上报判离线。
  上报分两条：**热列表**（近 7 天有活动的会话，附带最近对话）每 1.5s 全量重报一轮，
  实测 8 条 / 11 KB；**历史列表**（更老、仍在回溯窗口内的会话）每 30s 才带一次
  ——30 天窗口实测一轮 94 条 / 92 KB，若每 1.5s 全量重发就是每天约 5 GB 上行。
  hub 不落盘：重启后历史列表是空的，但它会在上报响应里带 `wantHistory` 主动索要，
  约一个上报周期（1.5~3 秒）即恢复，不必干等客户端那 30 秒定时器。任务/机器均带
  `hostname`、`platform`（macos / windows / linux）标识。

## 应用程序形态（Mac / Windows）

工作区分三个 crate：`core`（`am-core`，模型/进程/会话解析）、`hub`（`am-hub`，可执行名
`agent-task-monitor`，服务端）、`client`（`am-client`，可执行名 `agent-monitor`，桌面客户端）。
客户端是一个**托盘应用**：启动后驻留 macOS 菜单栏 / Windows 系统托盘，服务在后台运行。

- **macOS**：`bash scripts/package-macos.sh` 产出 `target/release/bundle/终端任务监控.app`
  （ad-hoc 签名，双击即用，不占 Dock），同时产出自更新用的 `target/dist/agent-monitor-mac.zip`。
  里面是 arm64 + x86_64 的 universal 二进制（`lipo` 合的），Apple 芯片与 Intel 共用一个包。
- **Windows**：`bash scripts/package-windows.sh` 产出 `target/dist/AgentMonitor-<版本>-setup.exe`
  （NSIS 中文安装向导，可选安装位置 / 桌面图标 / 开机自启；在 mac 上交叉构建需
  `cargo install cargo-xwin` 与 `brew install makensis`）。
- 无 GUI 环境（服务器/CI）：`AM_NO_TRAY=1` 或用 `--no-default-features` 编译纯服务版。
- hub 检测到前端构建产物（`AM_WEB_DIST` > exe 旁 `web/` > `../agent-monitor-web/dist`）时
  会直接托管，浏览器访问 `http://localhost:8383/portal` 无需单独起前端。

## 终端与项目分组

- 识别的宿主：**Cursor / VSCode 内嵌终端**（父进程链优先判 IDE，Windows 下不会被
  powershell/cmd 误判），以及**独立终端**——mac 的 Terminal/iTerm/Warp/kitty 等、
  Windows 的 Windows Terminal / PowerShell / CMD、tmux。
- 前台左侧顶部是一行独立的**设备选择器**（工具条之下、筛选框之上）：点开的下拉里每项写
  「主机名 · N 会话 · N 执行中」（离线设备写「离线」，执行中为 0 时不显示那一段），
  选中项存 localStorage、刷新后停在同一台；只有一台设备时退成静态标签（不可点、不进
  Tab 序）但仍然显示。数据源是 `/monitor/devices`，不从 `/monitor/tasks` 倒推 ——
  后者只含此刻有活进程的会话，拿它当设备表会让一台合上盖的笔记本整台消失。
- 下面的列表**只显示选中那台设备**的会话，按**客户端**分组（组标题就是 `Claude Code` /
  `Codex` / `ChatGPT 桌面版`，不再带主机名）。分组键是 `(provider, desktop)` 这一对而不是
  `provider` 一个值 —— 同一台机器上 Codex CLI 与 ChatGPT 桌面版都是 `provider="codex"`，
  却是两个不同的客户端，各占一组；直接吃 `/monitor/devices` 的 `providers` 字段。
  组内按最近活动分桶（今天 / 昨天 / 过去 7 天 / 过去 30 天 / 更早），列的是回溯窗口内的
  **全部会话**，已结束的也在，走 `/monitor/sessions/history` 的时间游标翻页、滚动加载；
  侧栏筛选框筛的就是这同一份全量（关键字一并传给历史接口）。
- 会话行在**执行中**时第二行直接显示当前动作（如「正在调用工具: Bash, Read」）。
- **子代理是智能体，不是会话**：它就地展示在对话正文的执行链里 —— 模型派活那一步画成
  一张**智能体卡**（目标 · 状态 · 个数 · 耗时），点卡里的子代理小卡，**那个子代理自己
  走过的执行链就作为兄弟节点原地接在卡片下面继续排**，不跳转、不弹层。子链默认铺 40 步，
  其余收进「展开全部」。卡片跑中默认展开、跑完默认收起，展开态不持久化。
- **左侧子会话树管定位，执行链管内容**，分工定死：侧栏的主会话可展开出它派过的子代理
  （名称 + 状态 + 耗时，默认收起、展开态持久化、超 20 条截断后给「展开全部」），点一条
  = 打开父会话并**滚到执行链里对应的那张智能体卡、自动展开它** —— 侧栏自己不渲染任何
  子会话正文，「复合 id `父::agentId` 当独立会话打开」那条路由已删除且不会恢复，同一份
  内容不留两个入口。定位靠 `subTask.toolUseId` 与链上 `tool.id` 配对，配不上的两种都给
  说法：没有 `toolUseId` 的置灰不可点；有 id 但那次调用不在已加载正文里的，在它那一行上
  标「定位不到」。树里**只列子代理**（`kind:"agent"`），后台命令在执行链上没有对应节点，
  仍在右栏看。
- 跑中的子代理**自动跟着长**，两条节律各管一件事：**子代理正文**仅当
  `outcome === "running"` 且子链正展开时每 5 秒拉一次（跑完先停定时器再补拉一次拿最终
  内容，收起 / 关格子 / 切走都随之停掉，终态永不轮询、拿到过就缓存）；**子任务清单**
  （`/subtasks`）在该会话还有 `running` 子代理时每 10 秒重拉一次，没有就一个请求都不发
  ——它是现读磁盘的，慢一档，代价是卡片最多晚 10 秒翻成终态。
- 前台右侧支持**拆分视图**：像 IDE 终端拆分一样并排显示最多 4 个任务，各自独立控制；
  每格底部是 hsu-ui `Chat.Input` 对话框，可**直接发布任务/终端命令**（按 provider 提供
  `/clear`、`/compact`、`/model` 等快捷键）。
- **状态右栏按格独立**：每格自己开关、自己记宽度，不再是全局共用一条；整格宽度不足
  640px 时摆不下，按钮置灰、栏不渲染。栏里只有 **todos 与后台命令**（`kind:"bg"`）——
  子代理不在这儿，它是执行链上的一步。
- 界面基准字号 14px，语义令牌（字号 / 颜色 / 圆角）直接引用组件库 `@hsu-react/ui` 的
  `--vita-*`，不再另抄一套字面值；全局下拉菜单几何统一走 `.va-menu`
  （用法 `rootClassName="va-menu"`）。

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
#    工作区里有两个可执行文件，必须用 -p 指明是服务端那个
cd agent-task-monitor
cargo run -p am-hub --release

# 2. 启动前端（端口 3003，/api 已代理到 8383）
cd ../agent-monitor-web
pnpm install && pnpm start
```

- 前台（需登录）：<http://localhost:3003/portal>，默认账号 `admin` / `admin123`，支持自助注册
- 后管（仅超级管理员）：<http://localhost:3003/admin>，登录后直接进入

## 安全机制

- **后管准入**：后管页面与全部 `/sys/*` 管理接口要求「已登录 + 超级管理员」
  （见 `hub/src/admin.rs` 的 `admin_gate`）。hub 首次启动仍会生成并持久化一个后管访问令牌到
  `~/.agent-monitor/admin-token`（可用 `AM_ADMIN_TOKEN` 覆盖），但已不再作为准入条件。
- **后管三页**：用户管理（查看/新增/改昵称/重置密码/删除注册用户）、版本管理（客户端版本与更新日志）、
  机器人接入（全局钉钉机器人，密钥不回显）；不记录任何日志类数据。
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
# macOS / Linux（agent 模式跑的是客户端 am-client，可执行名 agent-monitor）
AM_HUB_URL=http://<hub-ip>:8383 ./agent-monitor

# Windows (PowerShell)
$env:AM_HUB_URL="http://<hub-ip>:8383"; .\agent-monitor.exe
```

分发安装包时 hub 地址可以在编译期内置（`AM_DEFAULT_HUB_URL=... cargo build -p am-client --release`），
装完开箱即用，不必让用户配环境变量。跨平台编译：`cargo build -p am-client --release
--target x86_64-pc-windows-msvc`（在对应平台构建最简单）。

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
| `AM_HISTORY_DAYS` | `30` | 会话列表回溯天数（扫描多久以前的会话文件）。非数字 / 0 等非法值回落默认；进程内只读一次，改完要重启客户端 |

> `AM_HISTORY_DAYS` 与代码里的 `LIVE_WINDOW_MS`（固定 7 天）是两回事，改前者不会动后者：
> 前者决定**列表里能看到多久以前的会话**；后者决定哪些会话算「热」——参与会话 ↔ 进程配对、
> 并进每 1.5s 的热列表上报。放宽回溯窗口是为了看得见历史，放宽热窗口只会让配对判断变差。

> 桌面客户端启动时会用配置文件补齐没设的 `AM_*` 变量（环境变量优先），所以上表里的键
> 写进 `config.txt` 同样生效，不必设系统环境变量。每行 `KEY=VALUE`、`#` 开头为注释；
> 查找顺序：`AM_CONFIG` 指定的路径 > 可执行文件同级 `config.txt` > macOS `.app` 的
> `Contents/Resources/config.txt` > `~/.agent-monitor/config.txt`。

> ⚠️ 内置密钥仅供本地开发。对外部署请用 `openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048`
> 重新生成密钥对，私钥走 `AM_RSA_KEY_PATH`，公钥（SPKI base64）填入前端 `.env` 的 `RSA_PUB_KEY`。

## HTTP API（hub，8383）

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/monitor/tasks?status=&keyword=` | 任务列表（前台，平铺参数） |
| GET | `/monitor/tasks/page?query=` | 任务分页（后管，vita-admin Query 格式） |
| GET | `/monitor/tasks/detail/:id` | 任务详情 |
| GET | `/monitor/sessions/history` | 历史会话列表（**不过滤已结束**）。`keyword`（项目/标题/提示词/主机名）、`machineId`、`provider`、`desktop`（不传 = CLI 与桌面版都要）、`before`（时间游标）、`limit`（1–200，默认 50）；返回 `{list, total, nextCursor}`，每行带 `desktop` |
| GET | `/monitor/tasks/:id/messages?limit=` | 会话对话流。活跃会话读上报缓存，历史会话按需现读磁盘；返回 `{list, pending}` |
| GET | `/monitor/tasks/:id/subtasks` | 该会话**全部**子任务（现读磁盘、不套保留窗口，是快照里 `subTasks` 的超集）；返回 `{list, pending}` |
| GET | `/monitor/tasks/:id/subagents/:agentId/messages?limit=` | 单个子代理正文（`limit` 默认 200、上限 500）。`agentId` 取自 `subTasks[]` 里 `kind=agent` 且 `hasBody` 为真的条目 |
| GET | `/monitor/devices` | 设备列表。`machines[].providers: {provider, desktop, providerDsr, sessionCount}[]` 供侧栏建「设备 × 客户端」分组 |
| POST | `/monitor/tasks/:id/control` | `{action: pause/resume/interrupt/stop/kill, pid?}` |
| POST | `/monitor/tasks/:id/input` | `{text, pid?}` —— 向会话发布一行输入（TTY 注入） |
| GET | `/monitor/machines` | 机器列表（在线状态 / 系统 / 会话数） |
| GET | `/monitor/agent` | hub 自身状态 |
| GET | `/monitor/ws` | WebSocket 实时任务快照 |
| POST | `/monitor/report` | agent 上报（内部协议） |
| GET/POST | `/auth/access/*`、`/sys/menu/*` | vita-admin 登录 / 菜单 / 权限契约 |

> 上面几条带 `pending` 的接口：`pending: true` 表示 hub 已点名让那台机器现读磁盘、但结果
> 还没回来（最多等 8 秒）。调用方应显示「读取中」并重试，**不能当成空结果**。
> 现读磁盘是同步开销，大会话约 0.8–2 秒，**不要轮询**。
>
> **设备离线时怎么判**：历史会话的正文与子任务都在那台机器的磁盘上，hub 手里没有，机器
> 一离线就取不到。hub 的错误是 **HTTP 恒 200、真状态在 body 的 `code`** 上：`code=500`
> = 设备离线；但机器掉线约 8 秒后 hub 会把它名下的任务一并丢掉，此后同一种离线改以
> `code=404 任务不存在` 的形式出现。所以判离线**不能只看 code**，要配合 `/monitor/devices`
> 的 `online` 字段现场核对，否则会把一条好好躺在关机电脑上的会话说成「已被删除」。
>
> `/monitor/devices` 的 `providers`：一项 = 一个客户端，键是 `(provider, desktop)`，
> 所以**同一个 `provider` 可能出现两项**（`codex/false` = Codex CLI、`codex/true` =
> ChatGPT 桌面版）。`providerDsr` 是这对组合算出的**规范名**（`provider_dsr()` /
> `provider_dsr_desktop()` 两个固定枚举），不是客户端自由上报的字符串，也不取任何一条
> 会话的值 —— 取了的话 1 条桌面版会话就能把 32 条 CLI 会话的组改名。`sessionCount` 是
> **回溯窗口内**（`AM_HISTORY_DAYS`，默认 30 天）的会话数（含已结束、排除进程占位任务），
> **不是有史以来的总数**，口径与 `/monitor/sessions/history?machineId=&provider=&desktop=`
> 的 `total` 一致。排序由后端定好（会话数降序 → provider 名 → CLI 在桌面版前），前端不再
> 排一遍。字段恒存在，`[]` = 确实没会话。设备离线时照常返回上次已知的那份 ——
> 否则笔记本一合盖，侧栏分组会全部消失。
>
> 要取某一个客户端的会话，必须 `machineId + provider + desktop` 三件套一起传；只传
> `provider` 会把 CLI 与桌面版混在一起给你。热路径的 `desktop` 也都带：`/monitor/tasks`、
> `/monitor/tasks/detail/:id`、`/monitor/tasks/page`、WS 首帧 `{type:"tasks"}` 四处都有，
> 进程占位任务（id 含 `-pid-`，会话文件还没生成、判不出来源）恒为 `false`。
> **旧客户端上报的 `Task` 没有这个字段，按 `false` 取**，于是它的桌面版会话会并进 CLI 组
> ——这不是 bug，升级客户端即恢复正确分组。

## 扩展新代理（如 Codex）

1. `core/src/process.rs` 的 `agent_kind()` 已按进程名识别 `codex`；
2. 会话解析：仿照 `core/src/scanner.rs`（Claude 来源）为 Codex 的会话存储格式实现一个 Scanner，
   在 `client/src/state.rs` 的 `local_scan` 中合并两路 `SessionSummary` 即可；
3. 前端无需改动（`provider` 字段驱动展示与筛选）。

## 下载安装包

已构建好的桌面客户端安装包挂在 [GitHub Releases](https://github.com/VitaTsui/agent-monitor/releases)：

| 平台 | 产物 |
| --- | --- |
| macOS (Apple Silicon / Intel) | `agent-monitor-<版本>-macos-universal.zip` —— 解压得到 `终端任务监控.app`（universal 二进制，两种芯片通用） |
| Windows (x64) | `agent-monitor-<版本>-windows-x64.exe` —— NSIS 中文安装向导 |

每个产物配一份同名 `.sha256`，下载后可核对：

```bash
shasum -a 256 -c agent-monitor-<版本>-macos-universal.zip.sha256   # macOS / Linux
```

> Releases 上的包只作**备份下载**；客户端内的自动更新走自建服务器，与这里无关。
> macOS 包是 ad-hoc 签名，首次打开需在「系统设置 → 隐私与安全性」里放行。

## 贡献

日常开发在 `develop` 分支进行（feature 分支合入 `develop`），`main` 只接受来自 `develop` 的 PR。
PR 标题遵循 [Conventional Commits](https://www.conventionalcommits.org/)。

CI 会跑 `cargo fmt --check`、`cargo clippy`、`cargo test`（Linux / macOS / Windows 三端）
以及前端的 `pnpm lint` 与 `pnpm build`。`main` 上的版本号一变，`release` 工作流就按
`agent-task-monitor/Cargo.toml` 里的 workspace 版本打 tag 并建 Release，tag 已存在时安全跳过。

**clone 之后先挂上 git 钩子**（一次性，每个工作副本各做一次）：

```bash
git config core.hooksPath .githooks
```

`.githooks/pre-commit` 会在**本次提交暂存了 `.rs` 文件时**跑一遍
`cargo fmt --all -- --check`，不合规就拦下并告诉你跑什么命令修。只暂存前端文件时它
直接跳过，不会启动 cargo。不挂也能提交，代价是格式漂移要等 CI 变红才发现——
0.12.0 和 0.12.4 两次都是发版当场才现形、临时补了一刀格式提交。
临时要跳过用 `git commit --no-verify`；clippy 与测试仍由 CI 兜底，钩子不碰。

## License

[MIT](./LICENSE) © VitaHsu
