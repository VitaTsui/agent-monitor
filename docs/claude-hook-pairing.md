# 用 Claude Code hook 消灭「配对靠猜」

## 问题

会话（jsonl 文件）和进程（claude.exe）的对应关系，此前一直靠**推断**：

- 唯一权威信号 `CLAUDE_PID` / `CLAUDE_CODE_SESSION_ID` 只存在于 claude **临时派生的工具子进程**
  的环境变量里，跑完就没；空闲等待输入的会话一个都抓不到。实测同机 5 个 claude，8 个候选
  全部指向唯一那个正在执行工具的。
- 抓不到就退回 mtime 启发式 —— 同一时刻开的两个终端（pid 只差 8）极易配错。
- 表现：钉钉列表里标题串号（11 号显示 3 号的任务）、下发打到别的终端、网页内容对不上。

为此堆了一层层补丁：终端锚（shell pid + start）、pin 累积表、clear-follow 迁移层、
mtime 新鲜度兜底……全都是在给「猜不准」打补丁。

## 解法

Claude Code 的 hook 在 stdin 里**直接给出** `session_id` 和 `cwd`，环境变量里还有 `CLAUDE_PID`。
这是一条权威、及时、不用碰运气的信号——**让 claude 自己说它是谁**，就不必猜了。

客户端新增 `am-client hook` 子命令：从 stdin 读 hook JSON，把
`{claude_pid, session_id, cwd, at}` 落到 `<data_dir>/hooks/<pid>.json`。
扫描循环每轮读取，作为**最高优先级**的配对来源（覆盖 env pin 与句柄扫描）。

通道沿用 bridge 的思路：不引入本地服务，纯文件。

## 配置

在 `~/.claude/settings.json` 加（路径换成你本机客户端可执行文件的实际位置）：

```json
{
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "C:\\Users\\<你>\\AppData\\Local\\Programs\\AgentMonitor\\agent-monitor.exe hook"
          }
        ]
      }
    ],
    "PreToolUse": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "command": "C:\\Users\\<你>\\AppData\\Local\\Programs\\AgentMonitor\\agent-monitor.exe hook"
          }
        ]
      }
    ]
  }
}
```

- **`SessionStart`** 是关键：会话一开始就报上身份，不必等它执行工具。
- **`PreToolUse`** 是加保险：万一 SessionStart 那次没写成（客户端还没装好等），
  下次调工具会补上。`matcher: "*"` 匹配所有工具。

macOS/Linux 把 `command` 换成对应路径即可（例如 `/Applications/AgentMonitor.app/Contents/MacOS/agent-monitor hook`）。

## 安全性

hook 是**同步阻塞** claude 的，所以 `run_hook_cli` 的设计原则是「绝不失败、绝不拖慢」：

- 只做「读 stdin → 写一个小 JSON」，无网络、无扫描；
- 任何异常（空输入 / 坏 JSON / 缺 session_id / 目录建不了）都**静默退出 0**，
  宁可这次没记上，也绝不打断用户的会话；
- 原子写（先写 `.tmp` 再 rename），避免扫描循环读到半个文件。

已实测：正常输入正确落盘；空 stdin、非法 JSON、缺字段三种情况均退出 0 且不留脏文件。

## 记录的生命周期

- **TTL 12 小时**（`HOOK_REPORT_TTL_SECS`）：hook 只在起会话/调工具时写一次，之后会话空闲
  一整天那条配对依然成立，所以不能设太短，否则长时间挂着的会话反而失去最权威的信号。
- 进程已不在存活集合里 → 立即删掉该记录，防止 pid 重用后张冠李戴。
- 采纳时仍校验 `pid + start_time`，与终端锚同一套防重用逻辑。

## 怎么确认生效

看客户端日志：

```
配对来源 pinned=N 条（累积 · 本轮新增 X · hook自报=H env权威=E 文件句柄=F · 未配对=false）
```

`hook自报=H` 大于 0 就说明 hook 通了。理想状态是 `H` 等于本机 claude 进程数、`未配对=false`。

## 之后可以拿掉什么

hook 稳定覆盖后，这些补丁可以降级为兜底（**不要急着删**，hook 依赖用户完成配置，
未配置的机器仍要靠它们）：

- pin 累积表的「未配对就每轮扫 env」高频采集 → 可回落到低频
- mtime 启发式 → 仅在既无 hook 也无 pin 时使用
- clear-follow 迁移层 → `/clear` 后 SessionStart 会立刻报新 session_id，迁移推断不再必要
