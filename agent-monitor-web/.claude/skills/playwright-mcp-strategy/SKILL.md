---
name: playwright-mcp-strategy
description: Use this skill whenever the task will — or even might — end up driving a real browser via the playwright MCP tools (`mcp__playwright__browser_*`), AND also whenever the task is about to modify frontend source code that has any runtime effect in the browser (typical paths include but are not limited to `src/pages/**`、`src/components/**`、`src/layouts/**`、`src/stores/**`、`src/hooks/**`、`src/routes/**`、`src/App.tsx`、`src/main.tsx` 及任何 `.tsx` / `.jsx` / `.vue` / `.svelte` / `.module.less` / `.module.scss` / `.module.css` / 会被组件消费的 `.less` / `.scss` / `.css` / 前端 `.ts` / `.js` 源码) —— 这一类"前端源码改动"必须提前加载本 skill，才能在改完那一刻就按 skill 正文的"修改后的自动回归验证"章节决定要不要在浏览器里验一遍，而不是改完才想起来。This covers far more than "playwright E2E scripts": it also includes (1) any UI / frontend change where CLAUDE.md-style rules require starting the dev server and verifying the feature in a browser before reporting done; (2) reproducing a user-reported UI bug, regression, or "看起来不对" 的视觉问题; (3) visual / layout / chart / 样式 / 动画 / 响应式 / 暗黑模式 的实际渲染确认; (4) 登录/表单/跳转/弹窗/权限等交互流程的端到端验证; (5) 截图、snapshot、录屏、抓控制台报错、抓网络请求等"打开浏览器看一眼"的排查; (6) 爬取/抓取一个真实渲染后的页面内容; (7) 任意即将调用 `browser_navigate` / `browser_click` / `browser_snapshot` / `browser_take_screenshot` / `browser_evaluate` / `browser_fill_form` / `browser_console_messages` / `browser_network_requests` / `browser_verify_*` 等 playwright MCP 工具的操作; (8) 即将对前端源码文件发起 `Edit` / `Write` / `NotebookEdit` 等写操作 —— 无论是由用户明确要求，还是由 Claude 自己判断"该去浏览器里验证一下"或"我要改前端代码了"。典型中文触发短语包括但不限于："打开浏览器看看"、"在浏览器里验证"、"跑一下 playwright"、"截个图"、"snapshot 一下"、"E2E 测一下"、"端到端验证"、"复现这个 bug"、"看下实际效果"、"页面渲染对不对"、"图表出来了吗"、"样式生效了吗"、"点一下试试"、"登录进去看看"、"起个 dev server 验一下"、"帮我把这个功能在浏览器里跑通"、"改一下 XX 页面"、"改一下 XX 组件"、"调整 XX 样式"、"加一列 / 加一个按钮 / 加一个表单项"、"修一下这个前端 bug"、"重构这个组件"。英文同理："open the page"、"verify in browser"、"E2E test"、"reproduce in the browser"、"check the rendered UI"、"take a screenshot"、"run playwright"、"edit this component"、"tweak this style"、"fix this UI bug"。本 skill 定义了 snapshot 优先、screenshot 兜底、双重确认、截图落 `tmp/`、修改后的自动回归验证、以及配套 console/network/wait/verify 工具的使用规范，**必须在首次启动 browser MCP 前、或首次对前端源码发起写操作前主动加载**，以免"改完才意识到该验"。Skip only when the task is guaranteed to stay off the browser AND guaranteed not to touch any frontend runtime source — e.g. 纯后端/脚本/CLI 工作、纯文档/README/注释/CHANGELOG 改动、纯类型声明（`.d.ts`）无运行时效果、纯构建配置调整无运行时效果、纯静态文件或源码阅读、非 playwright 的 MCP 操作、或用户明确说"不用打开浏览器"/"这次不用验"。拿不准就加载，比漏加载便宜。
---

# Playwright MCP 使用策略

本 skill 规定通过 `mcp__playwright__browser_*` 工具操作浏览器时的默认流程，目标是：用最少的 token、最快的反馈，得到对页面状态最可靠的判断。

## 核心原则：snapshot 优先，screenshot 兜底

`browser_snapshot` 返回结构化的可访问性树（accessibility tree），带有可定位的 `ref`，是 LLM 友好的格式：可读、可解析、token 成本低、能直接喂给 `browser_click` / `browser_type` / `browser_fill_form` 等动作工具。

`browser_take_screenshot` 返回的是图像，token 成本高，且无法直接定位元素。它的价值在于"亲眼看到"页面，而不是"和页面交互"。

因此默认流程是：

1. **默认使用 `browser_snapshot`** 获取页面结构，基于 ref 进行点击、输入、选择、表单填写等操作。
2. **只有遇到下列情况，才追加 `browser_take_screenshot` 进行视觉确认。**

## 何时必须截图（snapshot 不够用）

满足以下任一条件，先调用 `browser_take_screenshot` 再分析：

- **需要验证视觉结果**：图表是否渲染出来、样式/颜色/布局是否符合预期、动画/过渡是否完成、暗黑模式是否生效、响应式断点是否正确。这些是像素级的判断，accessibility 树看不到。
- **snapshot 里找不到目标元素**：典型场景是 `<canvas>`（ECharts、Three.js、地图）、Shadow DOM 内的自定义组件、用 CSS 伪元素绘制的 UI、富文本编辑器内部结构。snapshot 对这些的可见度有限，必须靠截图判断"画出来了没有 / 长什么样"。
- **页面报错但 snapshot 无法定位问题**：例如点击后无反应、控制台报错但 DOM 看起来正常、出现意料之外的覆盖层（loading、modal、错误提示），需要靠截图看到完整的视觉状态再判断下一步。

## 关键操作的双重确认

涉及"是否成功"的判断时，**截图 + snapshot 双重确认**：

- 截图回答"用户视觉上看到了什么"
- snapshot 回答"DOM 里实际发生了什么 / 下一步可以点哪些 ref"

例如：提交表单后，截图确认 toast/跳转，snapshot 确认 URL/标题/列表是否更新。任何一方不一致都说明操作可能没真正生效，需要继续排查（看 `browser_console_messages`、`browser_network_requests`）。

## 修改后的自动回归验证（浏览器已开启时）

**前置条件**：项目的 playwright 浏览器已经处于开启状态——本会话已通过 `browser_navigate` 打开过目标页面、或上一次未执行 `browser_close`、或用户明确表示"playwright 开着"。满足该前置条件时，每完成一次代码修改，**默认自动走一遍回归验证**，不必等用户再次要求：

1. 根据本次修改的范围，导航/切 tab/触发交互到能看到该变化的页面状态；前端通常有 HMR，多数改动不需要刷新，但若改到路由/入口/环境变量等 HMR 覆盖不到的位置，显式 `browser_navigate` 当前 URL 触发刷新。
2. 用 `browser_snapshot`（视觉类改动叠加 `browser_take_screenshot`，图落 `tmp/`）确认修改后的行为、布局、数据是否符合预期。
3. 调用 `browser_console_messages` 查看控制台；若有报错：
   - 与本次修改直接相关 → **直接定位并修复**，修完回到步骤 1 再验一次。
   - 与本次修改无关但显然是问题 → 告知用户并询问是否一并处理，不要默默吞掉。
4. 若行为 / 视觉不符合预期，**自行继续修改 + 复验**，循环直到符合预期，或判断必须用户介入才停下。
5. 验证通过后，简短告知用户"已在浏览器里验过 + 控制台干净"，再收尾。

**何时可以跳过**：浏览器未开启；本次"修改"纯属文档/注释/纯类型/配置调整且无运行时效果；用户明确说"这次不用验"或"先不要打开浏览器"。拿不准就默认验一次，比漏验便宜。

**反模式**：
- ❌ 改完代码直接报告"已完成"，不碰浏览器——在浏览器开着的前提下属于偷懒。
- ❌ 只看 snapshot 不看 console，错过 React 警告 / 运行时异常 / 未捕获的 Promise。
- ❌ 控制台报错却绕过去继续做下一个任务——发现即修，除非用户另有指示。

## 反模式（不要这么做）

- ❌ 一上来就 `browser_take_screenshot`，把图喂回来再"在图上找按钮"——浪费 token、且无法点击。
- ❌ 每一步操作后都截图——绝大多数中间步骤靠 snapshot 判断就够了。
- ❌ 只截图不做 snapshot 就声称"操作成功"——视觉上的变化不代表 DOM 状态/数据真的更新了。
- ❌ snapshot 找不到元素时，反复换关键词重试——优先转去截图看清楚页面长什么样，再决定是等待加载、滚动、还是元素根本不存在。

## 与真实表单交互：不要用 DOM setter 塞值

受控表单（antd Form / hsu-ui `Form.Modal`）的值由 React 状态持有，DOM 上的 `value` 只是投影。用

```js
Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(el, "1");
el.dispatchEvent(new Event("input", { bubbles: true }));
```

去塞值**会制造出真实代码里并不存在的失败**，这一轮连踩三次：

- **绕过组件的值解析**：`INPUTNUMBER` 正常输入会交出 `null` 或数字，DOM 塞值留下的是字符串 `""` / `"1"`，提交直接 422 —— 看起来像业务 bug，实际是测试手法的产物。
- **被表单重置覆盖**：`Form.Modal` 有 `useEffect([form, value, open]) → resetFields() + setFieldsValue(value)`。塞值之后只要 `value` 引用变一次（比如 store 里 `resetFormData()` 赋了个新 `{}`），刚塞的值就被清空，表现为「必填项没填」。

**正确做法**：用 `browser_type` / `browser_fill_form`。拿不到 `ref` 时给 `browser_fill_form` 传 `selector`：

```js
selector: '.ant-modal-wrap:not([style*="display: none"]) input#title'
```

只有在**读取**页面状态、或触发按钮点击时才用 `browser_evaluate`。

## 判断弹窗是否关闭：看 wrap 的 computed display，别数节点

antd 的 Modal 关闭后 **DOM 节点仍在**，所以下面两种判据都会给出错误结论：

```js
// ❌ 关闭后 .ant-modal-content 依然存在，永远数得到
document.querySelectorAll(".ant-modal-content").length

// ❌ 缓存住的节点被移除后成了游离节点，computed display 仍是 block
const w = document.querySelector(".ant-modal-wrap");
/* …点击关闭… */ getComputedStyle(w).display   // 仍然 "block"
```

```js
// ✅ 每次重新查询，同时看 display 与 isConnected
const visible = () =>
  [...document.querySelectorAll(".ant-modal-wrap")]
    .filter((w) => getComputedStyle(w).display !== "none" && w.isConnected)
    .map((w) => w.querySelector(".ant-modal-title")?.textContent);
```

同理，**页面上同时存在多个弹窗时不要用 `document.querySelector(".ant-modal xxx")`**——它会命中第一个（往往是已关闭的那个）。先定位到目标 wrap，再在其内部查。关闭动画有几百毫秒，判定前留足等待（1.5s 起）。

## 「按钮点了没反应」先分三种情况

依次排除，不要直接猜是业务 bug：

1. **按钮本身 `disabled`**：`btn.disabled` —— 例如素材页的「新建自设数据」要先选中左侧条目才可用。
2. **被表单校验拦住，请求根本没发出**：查 `.ant-form-item-explain-error` 的文案，比翻 network 快。
3. **请求发了但被后端拒**：挂 XHR 钩子看请求体与状态码，尤其确认**字段类型**（`"1"` 还是 `1`）：

```js
const os = XMLHttpRequest.prototype.send, oo = XMLHttpRequest.prototype.open;
XMLHttpRequest.prototype.open = function (m, u) { this.__u = u; return oo.apply(this, arguments); };
XMLHttpRequest.prototype.send = function (b) {
  this.addEventListener("load", () => console.log(this.__u, String(b), this.status));
  return os.apply(this, arguments);
};
```

项目用 axios（XHR），**钩 `window.fetch` 抓不到任何请求**。

## hsu-ui Table 的列隐藏不移除节点

列显隐用 `width: 0.01` ＋ `.hidden`（`width: 0 !important`、内层 `display: none`）实现，**`<th>` 始终留在 DOM 里**。判断某列是否隐藏要量 `getBoundingClientRect().width`（隐藏后约 `0.0078px`），用「节点还在不在」判断会得出「列显隐不生效」的错误结论。

## 截图文件路径

调用 `browser_take_screenshot` 时，**必须显式传入 `filename`，把图片落到项目根下的 `tmp/` 目录**（例如 `filename: "tmp/binding-list.png"`）。原因：

- 仓库 `.gitignore` 已将 `tmp/` 排除，图片不会污染提交历史。
- 所有调试截图集中在一个位置，便于在会话结束后人工清理或复查。
- 不传 `filename` 会落到 MCP 的默认输出目录，散落在项目根或外部，容易遗留。

如果项目没有 `tmp/` 目录，先用 `mkdir -p tmp` 创建，再截图。不要把 PNG/JPEG 写到项目根、`src/`、或其它源码目录。

## 配套工具（按需调用）

- `browser_console_messages`：报错排查必看，先于猜测。
- `browser_network_requests`：表单/接口操作后确认请求是否真正发出且返回成功。
- `browser_wait_for`：等待文本/元素出现，避免 snapshot 拿到加载中状态。
- `browser_verify_*`：内置的断言工具，比手工比对 snapshot 更直接。

## 简明决策流程

```
要操作页面？
  ├─ 是 → browser_snapshot → 用 ref 操作
  │        └─ snapshot 找不到元素 / canvas / 自定义组件
  │             → browser_take_screenshot 看清楚 → 再决定下一步
  │
  └─ 要验证渲染/样式/图表？
       → browser_take_screenshot
       → 同时 browser_snapshot 确认 DOM/数据状态
       → 不一致就查 console + network
```
