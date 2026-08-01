# 从 Hermes Agent 可借鉴的实现 —— 评估

评估日期：2026-08-01
前提：本项目定位是**远程继续项目会话**（人不在电脑前，接着推进终端里那几个已有上下文的 Claude Code 会话），
不是"再造一个 agent"。下面每一项都按这个定位打分，Hermes 为它自身定位所做的设计不一定适合照搬。

信息来源：Hermes 官方架构文档（`hermes-agent.nousresearch.com/docs/developer-guide/architecture`）
与官方仓库。SEO 内容农场站点未采信。

---

## 一、总览

| # | Hermes 的做法 | 本项目现状 | 借鉴价值 | 工作量 |
|---|---|---|---|---|
| 1 | 长输出在聊天端截断/摘要，全量存库 | 截断 1500 字 + 完整内容发 `.txt` 附件，**无摘要** | ⭐⭐⭐ 高 | 中 |
| 2 | 会话血缘（parent/child）显式记录 | `/clear` 靠 clear-follow 每轮启发式重推 | ⭐⭐⭐ 高 | 中 |
| 3 | 审批/征询回调（callbacks.py，可中断） | 事后扫 jsonl 检测 `AskUserQuestion` 再推送 | ⭐⭐⭐ 高 | 中 |
| 4 | 平台适配器抽象（`platforms/*/delivery.py`） | dispatch 已渠道无关；**出站格式化散在各处** | ⭐⭐ 中 | 中 |
| 5 | 一个 Agent 类服务所有入口 | 已具备（`bot::dispatch` 服务钉钉 HTTP / Stream / 企微） | — 已有 | — |
| 6 | 原子写 + 竞争处理 | 已具备（tmp + rename） | — 已有 | — |
| 7 | SQLite + FTS5 跨会话检索 | JSON 文件 + 内存 | ⭐ 低 | 大 |
| 8 | 持久记忆 / skill 自建自改进 | 无 | ✗ 不建议 | 大 |
| 9 | 多 terminal backend（Docker/SSH/Modal…） | 无（刻意） | ✗ 与定位相反 | 大 |
| 10 | cron 定时任务 | 无 | ⭐ 低 | 小 |

---

## 二、建议采纳（按优先级）

### 1. 长输出摘要 —— 直接命中"手机上读结果"这个痛点

**Hermes 怎么做**：全量转录存 SQLite，聊天端只投递截断或摘要过的版本；`context_compressor.py`
做有损摘要时保留近期上下文与工具结果。

**你现在**：`server.rs:1879` 截断到 1500 字，超出部分作为 `.txt` 附件补发（`full_content`）。
机制已经在了，**缺的只是"摘要"这一步**——现在附件里是原始输出，手机上读一大段 diff 依然痛苦。

**建议做法**：在 hub 推送前，对超长的 `result()` 调一次小模型生成 3~5 行摘要，正文推摘要、
完整内容仍走附件。改动落在 `server.rs` 的 `result` 闭包附近，接口边界清晰。

**为什么值得先做**：你的核心场景是「人在手机上」，而现在最劝退的一环就是收到一大坨看不下去的输出。
这是唯一一处"借鉴 Hermes 的思路能立刻改善核心体验"的地方。

**注意**：要给摘要设超时与失败兜底（模型不可用时退回现有的截断行为），别让通知链路依赖外部服务的可用性。

### 2. 会话血缘显式记录 —— 从根上解决 `/clear` 配对

**Hermes 怎么做**：session 表显式记录 lineage（parent/child across compressions），
不靠事后推断谁取代了谁。

**你现在**：`/clear` 会让 claude 另起一个新 jsonl，`scanner.rs` 的 clear-follow 层每轮用
「同项目 + created 更晚 + cleared 标记」**重新推断**父子关系，还要靠 (a) 保持 / (b) 迁移两段
逻辑抑制抖动。今天那个「11 号显示 3 号标题」就是这类推断失手。

**建议做法**：改推断为记录——客户端一旦确认 `old_sid → new_sid` 的迁移，就把这条血缘落盘
（类似 `session-pairs.json`），后续直接查表。配合下面第 3 条的 hook，可以做到**零推断**：
claude 自己把 `session_id` 报上来。

**这一条与 hook 合并做收益最大**：`PreToolUse` / `SessionStart` hook 的 stdin 里直接带
`session_id` + `cwd`，加上环境变量里的 `CLAUDE_PID`，等于 claude 主动报告"我是谁、属于哪个会话"。
那套终端锚、pin 累积表、mtime 启发式全都可以退居兜底。

### 3. 审批/征询回调 —— clarify 拦截

**Hermes 怎么做**：`hermes_cli/callbacks.py` 注册 approval / sudo / clarification 回调，
工具需要审批时**暂停执行**并向用户抛出交互消息；平台侧用原生机制（emoji 反应、直接回复、
slash 命令）作答，且整个流程可中断。

**你现在**：`scanner.rs:1349` 解析 jsonl 里的 `AskUserQuestion` → `select` 角色 →
hub 推「⌨️ 需要你选择」+ 选项 → 你回「发 N 序号」。功能通了，但是**事后检测**：
写盘 → 客户端约 1.5s 扫一轮 → 上报 → hub 比对 → 推送，好几跳延迟。

**建议做法**：加 `PreToolUse` hook（matcher `AskUserQuestion`），在工具执行前把问题和选项
直接 POST 给客户端/hub，实现瞬时通知。回答仍走现有注入路径——因为 hook **不能代替工具返回结果**
（官方文档明确），只能 allow/deny/改输入。

**别做的**：不要用「阻塞 hook + deny 把答案塞进 `permissionDecisionReason`」来实现远程作答。
能跑通，但模型收到的是"工具被拒绝 + 一段解释"而非正常结果，行为不可预期，且阻塞期间整个终端卡住。

### 4. 出站格式化抽象 —— 只在你打算加平台时做

**Hermes 怎么做**：每个平台适配器有独立的 `delivery.py` 负责出站序列化，把统一响应转成平台原生格式
（富文本、按钮、embed）；平台差异只存在于入口，不渗进 agent。

**你现在**：入站已经是对的——`bot::dispatch(state, username, text, reply)` 渠道无关，
钉钉 HTTP 回调、钉钉 Stream、企业微信三个入口共用。但**出站**是散的：钉钉的 markdown 结构、
`{NO}` 占位、`.txt` 附件补发等硬编码在 `dingtalk.rs` 和 `server.rs` 的事件文案里。

**建议做法**：把"事件 → 平台消息"抽成一层（`NotifyEvent` 已经是这层的雏形，缺的是每平台的 renderer）。
**但如果短期内不打算加 Telegram / 飞书 / Slack，先别做**——现在只有钉钉真正在用，抽象出来是纯成本。

---

## 三、建议不采纳

| 项 | 理由 |
|---|---|
| **持久记忆 / skill 自建与自我改进** | 你不是 agent，没有"跨会话学习"的主体。真正的记忆在每个 Claude Code 会话自己的上下文里，你要做的是让人**接着用那个上下文**，不是另建一套。 |
| **SQLite + FTS5 跨会话检索** | 会话历史本来就在 `~/.claude/projects/*.jsonl`，claude 自己在管。再存一份是重复数据源，还要处理同步与一致性。真需要搜索时，直接检索 jsonl 更省事。 |
| **多 terminal backend（Docker/SSH/Modal/Vercel Sandbox）** | 与定位**正相反**。Hermes 要的是可丢弃的沙箱；你要的恰恰是用户那台机器上、带着 git 工作区和项目上下文的真实终端。 |
| **subagent 并行编排** | 你已经有更自然的等价物：多个真实终端会话 + 号位寻址 + MCP 六件套。再造一层调度是重复。 |
| **cron 定时任务** | 边际收益低。真需要"定时看一眼"，现有的钉钉「会话」指令加手机快捷方式就够。 |

---

## 四、优先级建议

按「对**远程继续项目会话**这件事的改善」排序：

1. **hook 报告会话身份**（第 2、3 条的共同基础）——一次投入，同时解决配对不准和 clarify 延迟，
   收益最大且最确定。你这两天所有配对相关的 debug 都是在给这个问题打补丁。
2. **长输出摘要**（第 1 条）——直接改善手机端体验，机制已有底子，改动局部。
3. **会话血缘落盘**（第 2 条）——hook 落地后，这条大部分自然解决；剩下的兜底再补。
4. **出站格式化抽象**（第 4 条）——等真要加第二个平台时再做，现在做是提前优化。

---

## 五、对前一份文档的修正

`hermes-complementarity.md` 第六节把"做互补（接 Hermes 的 MCP）"排在第一位，那是在你的核心诉求
尚未明确时写的。诉求确认为「远程继续项目会话」之后，那个排序偏了：

- MCP 端点**值得留着**（成本已付，任何 MCP 客户端都能接），但不该是投入重点；
- 真正的重点是本文档第四节的 1、2 两项——把「认得出会话、发得进去、看得懂结果」这条链路走顺。

---

## 参考

- [Hermes Agent · Architecture](https://hermes-agent.nousresearch.com/docs/developer-guide/architecture)
- [NousResearch/hermes-agent（GitHub）](https://github.com/NousResearch/hermes-agent)
- [Claude Code · Hooks](https://code.claude.com/docs/en/hooks)
- 本项目相关实现：`core/src/scanner.rs`（clear-follow、AskUserQuestion 解析）、
  `hub/src/server.rs`（事件与推送）、`hub/src/bot.rs`（渠道无关分发）、`hub/src/mcp.rs`
