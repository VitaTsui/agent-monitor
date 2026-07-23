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
- **pinned（最可靠）**：`client/src/openfiles.rs::pin_windows` 用 Restart Manager `RmGetList`
  查「哪个进程持有这个 jsonl 文件句柄」。**实测 `pinned=0`（见下）。**
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

**0.7.42 刚做的事（待验证）**：`openfiles.rs::RECENT_MS` 从 6h 放宽到 **8 天**
（`pin_windows` 原来只检查「6h 内改过的 jsonl」，把 2 天闲置的会话文件排除在外，压根没查 holder
→ 自然 pinned=0）；同时 pin 频率从每 4 轮降到 **每 20 轮**（`state.rs:315`，约 30s）省开销。

## 5. 下一步决策树（**从这里继续**）

让用户升级到 **0.7.42**，让某个 Cursor 会话**正在生成输出时**看 `client.log` 的 `配对来源 pinned=?`：

### 分支 A：`pinned ≥ 1`（claude 确实持有闲置会话文件句柄）
→ pinned 现在能命中。**修法**：让配对优先用 pinned（`build_tasks` 已有 pinned 优先级，只要
`pin_windows` 现在能返回非空即可），配对缓存（PREV_PAIRS）会把正确配对粘住。基本就修好了。
可能还要：确认 `pin_windows` 检查的是「有活进程的项目」的 jsonl（见性能注意）。

### 分支 B：仍 `pinned = 0`（即使正在生成）
→ claude 在 Windows 上**写一行开一次、写完就关**，Restart Manager 抓不到任何持有者。
时间戳无解。需要**更重的开放句柄扫描**：
- `NtQuerySystemInformation(SystemHandleInformation)` 枚举**全系统句柄** → 找 file 类型句柄 →
  `DuplicateHandle`（从目标进程 dup 到本进程，需 `PROCESS_DUP_HANDLE`）→ `GetFinalPathNameByHandleW`
  解析路径 → 匹配 jsonl → 得 pid↔session。
- **坑**：`NtQueryObject` 对某些同步句柄会**挂起**，需另起线程加超时；要按 ObjectTypeIndex 过滤
  file 句柄；开销大，要节流 + 只查有活进程的项目的 jsonl。
- 若开放句柄扫描也找不到（claude 真的不持有），则**文件层面无解**，只能考虑：
  让扩展在 Node 里枚举每个终端 shell 的子进程树拿到 claude pid（给出可靠的 **claude_pid↔terminal**），
  但**仍缺 session↔claude_pid** —— 除非能从 claude 进程读到它的 session id（命令行没有；
  可能得读进程环境变量 PEB / 或 claude 是否在某处落 session 标记，需调研 claude Code 行为）。

**优先做分支判断，别在 B 上盲写不可测的 unsafe。** 我（上一手）一直没能在本机（mac）测 Windows 代码，
所以每次都靠日志验证——请沿用「改一点 → 出诊断日志 → 让用户回传 → 再定」的节奏。

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
4. 部署到 `root@***REMOVED***`（hostname vultr，服务 User=agentmon）：
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
| pinned（RmGetList） | `client/src/openfiles.rs::pin_windows / holder_pids`，`RECENT_MS` |
| pin 频率 + 配对缓存 + pinned 诊断日志 | `client/src/state.rs`（`tick % 20`、`PREV_PAIRS`、`配对来源`） |
| 文件桥 | `client/src/bridge.rs` + `agent-monitor-vscode/src/extension.ts` |
