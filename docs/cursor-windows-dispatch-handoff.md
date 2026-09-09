# 交接文档：Windows 上 Cursor/VSCode 任务下发「配错终端」的修复

> 目标读者：接手继续修这个 bug 的 Claude Code。读完本文即可直接动手，不用回溯历史会话。
> 当前版本：**0.7.42**（`agent-task-monitor/Cargo.toml` 的 `[workspace.package] version`）。

## 1. 一句话问题

Windows 上，网页里对某个 **Cursor/VSCode 内嵌终端会话** 下发任务（如 `/clear`），
**经常发到错误的终端**。同一个 Cursor 项目里并发开了多个 claude 会话时尤其明显、且**随机**，
肉眼看不出规律。

根因**不在下发链路本身**，而在**「会话(jsonl) ↔ claude 进程」的配对**：配对错了，后面一切都错。

## 2. 下发链路（Windows，三条路，已经能工作）

网页选中会话 → `sendInput(taskId, text, task.pid)` → hub 入队 `ControlCmd{Input, pid, text}`
→ 客户端 `client/src/agent.rs::execute()` 按 `pid` 决定怎么送达：

`execute()` 里对 Input 的分发（`client/src/agent.rs:~483`）：
1. **Cursor/VSCode 内嵌终端**：`am_core::process::ide_shell_pid(pid)` 沿父链找到内嵌终端的
   **shell pid**（= 扩展里 `terminal.processId`）。若有活着的桥接扩展在管这个终端
   （`bridge::has_live_terminal`），就写文件桥 `bridge::send_via_extension` 交给扩展
   `terminal.sendText` 送达。**这一步依赖 `pid` 是「该会话真正的 claude 进程」——配对错这里就错。**
2. **Windows Terminal**（`windows_send_input` 里先判 `windows_wt_pid`）：ConPTY 写不进，改
   「聚焦 WT 窗口 + 剪贴板粘贴 + 回车」（`windows_paste_send`）。
3. **传统 conhost 控制台**：`WriteConsoleInput`（0.7.32 已改**每 8 条分批** + `WriteConsoleInputW`
   Unicode，修 `0x8007007A` 缓冲区不足）。

关键文件/函数：
- `core/src/process.rs`：`windows_send_input:587`、`ide_shell_pid:708`（跨平台，走父链找
  cursor/code 之下最近的 shell pid）、`windows_wt_pid:749`、`windows_paste_send:771`。
- `client/src/bridge.rs`：文件 IPC。`<data_dir>/bridge/`：扩展写 `win-<id>.json` 心跳
  （{ts, terminals:[shell_pid...]}）；客户端写 `outbox/<ts>-<pid>.json`（{pid,text,ts,submit}）。
- `agent-monitor-vscode/`：VS Code/Cursor 扩展（`src/extension.ts`）。轮询 outbox，pid 命中
  自己某个终端的 `processId` 就 `sendText`，并记 `bridge/ext.log`。客户端默认自动装它
  （`desktop.rs::ensure_bridge_extension`，从 hub 下 `agent-monitor-bridge.vsix`）。

**下发链路本身没问题**：`ide_shell_pid(C)` 对给定 claude 进程 C 能可靠算出它的终端 shell pid；
扩展按 `processId` 匹配也可靠。**唯一的错误来源是传进来的 `pid` 是不是该会话真正的 claude 进程。**

## 3. 真正的病灶：会话↔进程配对（`core/src/scanner.rs::build_tasks`）

`build_tasks`（`scanner.rs:503`）把「会话摘要 `SessionSummary`」和「claude 进程 `ProcessInfo`」
配成 `Task`（带 `pid`）。配对按优先级：

- **① 命令行 `--resume <id>`**：命令行带会话 id → 精确。但用户的 claude 是
  `claude.exe --dangerously-skip-permissions`（无 --resume），**用不上**。
- **pinned（最可靠）**：会话 ↔ claude 进程的**权威链**。
  - ~~旧法：`openfiles.rs::pin_windows` 用 Restart Manager `RmGetList` 查文件句柄持有者~~
    —— **实测无效**（见 §4/§5，已默认禁用）。
  - **新法（2026-07-23 落地）**：`core/src/process.rs::ProcessScanner::session_pins` 从 claude
    **派生子进程的环境变量** `CLAUDE_PID` + `CLAUDE_CODE_SESSION_ID` 直接取「claude pid ↔ 会话 id」。
    `state.rs` 把它喂进 `pinned`（env 优先，句柄扫描作补充）。`build_tasks` 已优先用 pinned。
- **② created≈start**：进程一定先于它创建的会话，故会话 `created_ms ≥ 进程 start_time`。
  0.7.41 已把「回看窗口」收到 **500ms**（`CREATE_BACK_MS`，`scanner.rs:613`），并改成
  **按进程启动升序、各认领最早的后继会话**（不再用 |diff| 最小贪心，那会让晚开进程抢走早开
  进程的会话 → 「发到上一个会话终端」）。**但对 resumed 会话无效**（created 远早于 start）。
- **③ `--continue`** / **④ 最近 30min mtime + mtime≥start**：resumed/长会话靠 ④，
  按 mtime 匹配，**并发同项目会话时本质上有歧义**（就是现在「随机错位」的来源）。
- **⑤ 配对缓存**（0.7.37，`state.rs::PREV_PAIRS`）：上一轮配对兜底，让长闲置会话保持配对不
  掉成占位。**但缓存只是稳定化，配错了会一直错。**

**结论**：`pinned=0` + resumed 会话（无 created≈start 线索）→ ②④ 都不可靠 → 随机错位。
**没有权威的「会话↔进程」链，任何时间戳启发式都救不了。**

## 4. 已排查到的关键事实（来自用户 Windows client.log）

日志位置：`%APPDATA%\AgentMonitor\client.log`，**必须 `Get-Content ... -Encoding UTF8`** 读否则乱码。
`bridge/ext.log` 记扩展匹配到的终端。

诊断行（`state.rs:339` 每 ~30s 一条）：
```
配对来源 pinned=0 条：{}（进程数 6）
桥接判定：会话 0d86f9e2-… claude pid=44416 → 内嵌终端 shell pid=35548，扩展在管=true
注入输入：经 Cursor/VSCode 扩展桥接（终端 pid=35548，/clear…）
```
实测：
- **6 个 claude 进程**，4 个在 25s 内先后启动（`start_age` 164840→164815s）。
- 会话都是 **resumed/闲置约 2.2 天**（`mtime_age≈192000s`，`created_age` 更早），并非当前进程新建。
- **`pinned` 一直是 0**。

**已验证（2026-07-23，真机）**：放宽 `RECENT_MS` 到 8 天后再全量扫描，holder 仍 0 —— 确认
「闲置会话被 claude 持有句柄」这个前提**为假**，句柄扫描此路不通（见 §5）。已改走环境变量方案。

## 5. 决策树（已解决 —— 2026-07-23，在真机 Windows 上直接实证）

> 上一手在 mac 上测不了 Windows；这一手在 Windows 本机装了 MSVC，直接 build/run/实测，决策树已跑到底。

**分支判断结果 = 分支 B（且比原判更彻底）。** 用 Restart Manager 直接采样实证：
- 30s × 25ms 对 10 个 jsonl 采样约 12000 次，任意时刻「被占用」= 0；
- 8 天窗口 120 个候选 jsonl 全量 RmGetList 扫描，holder 恒 **0 / 120**；
- 唯一捕获的写入瞬间，排他 open(FileShare.None) 仍成功。

→ **claude 写一行开一次就关，句柄寿命 < 25ms、不跨空闲持有。**

### 原分支 A（pinned≥1 靠句柄）——不成立
RM 永远抓不到。已把 `pin_windows` 默认早退（`AM_PIN_RM=1` 可恢复做诊断），省掉每轮最多 200 次
RmGetList 的空转开销。

### 原分支 B 的「开放句柄扫描」——**也是死路，别做**
`NtQuerySystemInformation(SystemHandleInformation)` 枚举的是**当前打开**的句柄，同样抓不到 <25ms
的瞬态句柄；还有 `NtQueryObject` 挂起风险 + 大开销。做了也修不了配对。

### ✅ 真正的解法（已落地）：环境变量给出权威链
实测发现 Claude Code 给它**派生的每个子进程**注入两个环境变量：
```
CLAUDE_PID = <拥有该子进程的 claude 进程 pid>
CLAUDE_CODE_SESSION_ID = <会话 jsonl 文件名>
```
（claude 自身 env 里没有这两个，只有 `CLAUDE_CODE_SSE_PORT`/`CLAUDE_CODE_ENTRYPOINT`；所以要读的是
**子进程**的 env，不是 claude 进程自己的。）据此得「会话 ↔ claude pid」的**权威链**，不靠句柄、不靠时间戳，
从根上消除并发同项目会话的歧义。

落地：`ProcessScanner::session_pins`（`core/src/process.rs`）用 sysinfo 的 `environ()` 扫全部进程，
抽出 `CLAUDE_PID→CLAUDE_CODE_SESSION_ID` → `state.rs` 喂进 `pinned`（每 20 轮节流，env 优先于句柄扫描）
→ `build_tasks` 优先用 pinned，tier⑤ 缓存把它粘住。**无需 unsafe**（sysinfo 内部读 PEB）。

**已知覆盖限制**：`session_pins` 只在会话**有活着的子进程**（正在跑工具/命令）时能取到；空闲会话没有
子进程 → 取不到，交回配对缓存（tier⑤）+ mtime 兜底。好在：① 用户下发/会话活跃时正是有子进程的时刻；
② 活跃时拿到过一次，缓存就粘住。若要覆盖「从头到尾空闲、且没缓存」的会话，未来可让 bridge 扩展在
Node 侧上报 `terminal.processId ↔ CLAUDE_CODE_SESSION_ID`（扩展能读到自己终端子进程的 env）。

**验证**（真机，2026-07-23）：`cargo test -p am-core` 34/34 绿；client 实跑 `配对来源 pinned=… env权威=…`
诊断行确认 env 链命中（如 `CLAUDE_PID=3744 → 1e71cdfa…`）。诊断读法：`Get-Content client.log -Encoding UTF8`。

## 6. 构建 / 部署 / 验证（务必照做，踩过坑）

**编译校验**（改 core/client 后）：
```bash
cd agent-task-monitor
cargo check -p am-core -p am-client                                   # mac
cargo xwin check -p am-client --target x86_64-pc-windows-msvc         # windows（#[cfg(windows)] 代码只有这里查得到）
cargo test -p am-core --lib                                           # 33 个配对测试必须绿
```

**发版**（改了会影响客户端行为就要发；桌面自更新版本 = hub 自身编译版本）：
1. bump `Cargo.toml` 的 `version`。
2. `cargo zigbuild -p am-hub --release --target x86_64-unknown-linux-musl`（bin 名 `agent-task-monitor`）。
3. `bash scripts/package-macos.sh`（出 `agent-monitor-mac.zip`）、`bash scripts/package-windows.sh`（出
   `AgentMonitor-<v>-setup.exe`，务必核对约 10.7MB，太小是 makensis 半包）。
4. 部署到 `<SSH_USER>@<PROD_HOST>`（服务 User=agentmon）：
   - **hub 必须覆盖到 systemd ExecStart 指向的 `/opt/agent-monitor/agent-task-monitor`**（别传成 `am-hub`！），
     用 `mv`（`cp` 会 ETXTBSY），chown agentmon，`systemctl restart agent-monitor`。
   - `/opt/agent-monitor/data/downloads/` 放：`agent-monitor-mac.zip`、**`agent-monitor-setup.exe`（固定名，
     Windows 自更新下的就是它——漏了会导致更新循环）**、`AgentMonitor-<v>-setup.exe`（版本化，广播触发器，最后传）。
     扩展改了还要更新 `agent-monitor-bridge.vsix` 并 bump `desktop.rs::BRIDGE_EXT_VERSION` + 扩展 `package.json` version。
5. 校验：`curl -s http://127.0.0.1:8383/monitor/version` 的 `desktop` = 新版；
   `cmp -s agent-monitor-setup.exe AgentMonitor-<v>-setup.exe` 一致。
6. 重启 hub 后确认钉钉 Stream 重连：`journalctl -u agent-monitor --since "40 sec ago" | grep 钉钉`。

**测试**：用户升级客户端后，对并发的多个 Cursor 会话逐个发 `/clear`，看是否各自命中自己终端；
回传 `client.log`（`-Encoding UTF8`）的 `配对来源` / `桥接判定` 行 与 `bridge/ext.log`。

## 7. 相关记忆（memory/）

- `agent-monitor-termkey-injection.md` — 各终端注入支持度 + Windows client.log 排查法 + 0x8007007A 分批修 + EcoQoS 节流。
- `agent-monitor-desktop-release-flow.md` — 发版完整流程 + 两个实际踩过的部署坑（hub 传错名、固定名 win 安装包漏传）。
- `agent-monitor-workspace-split.md` / `agent-monitor-queue-operation-semantics.md` — 工作区结构 / 排队语义。

## 8. 快速定位清单

| 关注点 | 位置 |
|---|---|
| 下发分发（IDE桥/WT/conhost） | `client/src/agent.rs::execute()` Input 分支 |
| 内嵌终端 shell pid | `core/src/process.rs::ide_shell_pid` |
| 会话↔进程配对（核心病灶） | `core/src/scanner.rs::build_tasks` 阶段①-⑤ |
| pinned（**env 权威链**，主力） | `core/src/process.rs::ProcessScanner::session_pins`（CLAUDE_PID/CLAUDE_CODE_SESSION_ID） |
| pinned（句柄扫描，已默认禁用） | `client/src/openfiles.rs::pin_windows / holder_pids`（`AM_PIN_RM=1` 可恢复诊断） |
| pin 频率 + 配对缓存 + pinned 诊断日志 | `client/src/state.rs`（`tick<=1 \|\| tick%20`、env 优先合并、`配对来源 … env权威=…`） |
| 文件桥 | `client/src/bridge.rs` + `agent-monitor-vscode/src/extension.ts` |
