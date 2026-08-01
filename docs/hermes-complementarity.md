# agent-monitor × Hermes Agent 互补评估

评估日期：2026-08-01　　结论先行：**互补成立，且接口已经就绪；但"该不该做"取决于你把自己定位成什么。**

---

## 一、事实基础

信息来自 Hermes 官方仓库与文档（`github.com/NousResearch/hermes-agent`）。
搜索结果里 `hermesatlas.com` / `hermes-ai.net` / `petronellatech.com` 一类站点疑似 SEO 内容农场，本文未采信。

| | Hermes Agent | agent-monitor |
|---|---|---|
| **本质** | 自托管的独立 AI agent | 给**既有终端会话**加的远程可观测 + 可控层 |
| **谁在干活** | Hermes 自己的进程 | 你亲手在终端里开的 Claude Code |
| **执行环境** | 自己 spawn（Docker / SSH / Modal / Vercel Sandbox…） | 不 spawn，附着到你已经开着的进程 |
| **上下文** | 自带记忆库、context files | 你的项目现场：CLAUDE.md、git 工作区、做到一半的事 |
| **记忆** | 持久化、跨会话、自我改进 skill | 无（也不需要） |
| **IM 入口** | Telegram / Discord / Slack / WhatsApp / Signal / Email 等 20+ | 钉钉、企业微信 |
| **编排** | subagent、cron、skill 自建 | 号位寻址 + MCP 六件套 |
| **规模** | 66k★，2026-02 发布，迭代极快 | 个人项目 |

**关键结论**：两者不在同一层。Hermes 是"另一个 agent"，agent-monitor 是"你现有 agent 的远程遥控器"。
Hermes 明确 spawn 自己的沙箱执行环境，**不接管用户手动开的终端会话**——这正是留给 agent-monitor 的空间。

---

## 二、互补点在哪

Hermes 缺的恰好是 agent-monitor 唯一擅长的：**触达用户真实的工作现场**。

Hermes 能在 Docker/SSH/Modal 里跑命令，但那是一次性沙箱：没有你的 git 工作区状态、没有你项目的 CLAUDE.md、
没有你那个已经聊了 50 轮、装着完整上下文的 Claude Code 会话。而这恰恰是真实开发工作最值钱的部分。

反过来，agent-monitor 缺的正是 Hermes 的强项：记忆、跨会话学习、编排调度、多 IM 平台覆盖。

```
                   ┌─────────────────────────────────┐
   Telegram/       │          Hermes Agent           │
   Slack/... ─────▶│  记忆 · skill · cron · 编排调度  │
                   └───────────────┬─────────────────┘
                                   │ MCP over HTTP
                                   ▼
                   ┌─────────────────────────────────┐
                   │   agent-monitor hub  /mcp       │
                   │   list / detail / send /        │
                   │   wait_until_idle / control     │
                   └───────────────┬─────────────────┘
                                   │ 上报 + 命令下发
                   ┌───────────────┴─────────────────┐
                   ▼               ▼                 ▼
              终端 #1 claude   终端 #3 claude    终端 #9 claude
              （你的项目现场，随时可切回接管）
```

---

## 三、可行性：接口已经就绪

**Hermes 内置 MCP 客户端，支持 stdio / HTTP / StreamableHTTP，自动发现工具并注册成原生工具。**
agent-monitor 刚实现的 `/mcp` 正是 Streamable HTTP——不需要为对接再写任何适配代码。

Hermes 侧配置（`~/.hermes/config.yaml`）：

```yaml
mcp_servers:
  agent_monitor:
    url: "https://monitor.vita-llm.com/mcp"
    headers:
      Authorization: "Bearer <登录 token>"
    tools:
      # 官方建议：敏感系统用白名单，别 connect everything
      include: [list_sessions, session_detail, send_to_session, wait_until_idle]
      resources: false
      prompts: false
```

改完执行 `/reload-mcp` 生效。

**强烈建议按上面那样用白名单**，把 `control_session`（含 `stop` 终止进程）和 `recall_last` 排除在外。
Hermes 文档自己也强调"connect the right thing, with the smallest useful surface"，
对破坏性操作要用 allowlist 而非 exclude。让远程 agent 能终止你正在跑的会话，风险与收益不成比例。

### 能立刻跑通的闭环

```
你（在 Telegram 上）："看下我台式机上那几个项目的进度，把卡住的挑出来"
  Hermes → list_sessions          → 拿到 1/3/9 号及状态
  Hermes → session_detail(3)      → 读最近对话，判断卡在哪
  Hermes → 汇总回你

你："让 9 号把 README 补完"
  Hermes → send_to_session(9, ...) → 真的注入进你那个终端
  Hermes → wait_until_idle(9)      → 等它干完
  Hermes → session_detail(9)       → 取结果回报
  （你回到电脑前，看到的是终端里完整的执行过程，可以直接接管）
```

这个闭环里，Hermes 出记忆与调度，agent-monitor 出手脚，两边都在做自己最擅长的事。

---

## 四、要补的东西

按性价比排序：

| 项 | 说明 | 工作量 |
|---|---|---|
| **修 `recall_last`** | 现版本无条件发 `TermKey up:1`，hub 侧还有 pending 时行为是错的（应直接丢弃 pending）。应复用 `bot::recall_last` 的完整逻辑 | 小 |
| **只读 token / 作用域** | 现在 MCP 用的就是网页登录 token，等于把账号全权交给 Hermes。应支持签发**受限 token**（只读、或限定若干会话） | 中 |
| **工具级审批** | 对 `send_to_session` 这类写操作，可考虑要求钉钉侧二次确认（复用刚做的冷却确认机制） | 中 |
| **`wait_until_idle` 的长连接** | 现在最多占住 120s HTTP 请求。Hermes 若超时更短会失败；可改为返回"轮询建议"由调用方重试 | 小 |
| **SSE 支持** | `GET /mcp` 现返回 405。规范允许，但个别客户端坚持要 SSE 流 | 小 |

---

## 五、风险与不确定性

- **未实测**。上述配置基于 Hermes 官方文档推导，尚未真的把两者接起来跑通。第一步应该是拿一个测试会话验证闭环。
- **安全面扩大**。`/mcp` 用的是网页登录 token，一旦泄露等于整个账号失守；而 Hermes 部署在另一台机器上、由另一个 LLM 驱动。在补上受限 token 之前，不建议把这个能力开给自己以外的任何人。
- **提示注入**。Hermes 读到的 `session_detail` 内容来自你的终端会话，其中可能包含代码/网页抓取的文本；理论上存在借内容操纵 Hermes 去调 `send_to_session` 的路径。白名单排除破坏性工具能显著降低危害。
- **Hermes 迭代极快**（两个月七个大版本），配置格式和 MCP 行为可能变。别把对接做得太紧耦合。
- **依赖关系反转的风险**：一旦习惯从 Hermes 侧发起，agent-monitor 就退化成一个 MCP 后端，钉钉入口、网页、移动端这些自有界面的价值会被稀释。这是产品层面的取舍，不是技术问题。

---

## 六、建议

**做互补，但不要转型。**

1. **短期（值得做）**：修掉 `recall_last`，加只读/受限 token，然后按第三节配置实测一遍闭环。
   成本很低（接口已就绪），收益是让 Hermes 用户多一个"能碰真实工作现场"的理由——这是你独有的。

2. **中期（看情况）**：把 MCP 端点当成对外的正式接口来维护（文档、版本、受限 token），
   而不只是"顺手加的一个 endpoint"。这样任何 MCP 客户端都能接，不只是 Hermes。

3. **不要做的**：不要去追 Hermes 的记忆、skill、多平台网关。那是它两个月七个版本、66k★ 在做的事，
   个人项目正面对抗只会稀释你真正的差异点。

**判断标准还是那句**：如果你的核心诉求是"有个 AI 助手随时能找我干活"，Hermes 已经做得更好，
你在重复造轮子；如果是"我人不在电脑前，也要能推进电脑上正跑着的那几个项目"，
那没人替代你——你这两天 debug 的配对问题（jsonl ↔ claude 进程 ↔ 终端窗口、终端锚、pin 累积、
clear-follow 迁移）正是这个定位独有的难点，Hermes 根本不需要解决它。

---

## 参考

- [NousResearch/hermes-agent（GitHub）](https://github.com/NousResearch/hermes-agent)
- [Hermes Agent · MCP 功能文档](https://hermes-agent.nousresearch.com/docs/user-guide/features/mcp)
- [Hermes Agent · Use MCP with Hermes（配置与安全建议）](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/guides/use-mcp-with-hermes.md)
- 本项目 MCP 实现：`agent-task-monitor/hub/src/mcp.rs`
