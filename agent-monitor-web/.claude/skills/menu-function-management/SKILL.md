---
name: menu-function-management
description: Use this skill ONLY when the user asks to add, modify, or delete a 菜单 (menu) or 功能 (function/permission) entry through an existing backend admin "菜单管理 / Menu management" page in the running application — i.e. the operation happens in the rendered UI, not by editing source code. The skill ALSO covers the immediate follow-up step of going to the 角色管理 / Role management page and granting the newly-added menu/function to 超级管理员 (super admin) so the changes are actually visible to anyone. Typical trigger phrases include 中文 ("帮我加一个菜单"、"菜单管理里新增一项"、"加个功能权限"、"菜单管理里改一下 XX 的图标"、"在 XX 菜单下加一个新增/修改/删除功能"、"补一个权限标识"、"加一个一级菜单"、"给超级管理员配上新菜单的权限") and English ("add a menu", "add a permission/function under X", "edit the icon of menu X", "create a CRUD function set under X menu", "grant the new menu to super admin"). HARD PRECONDITION: the current project must have a working 菜单管理 page reachable from the running app (e.g. permit/Menu, system/Menu, sys/Menu, admin/menu, RBAC menu admin); if no such page exists in this project, DO NOT run this skill — tell the user the project has no menu admin page and stop. Skip this skill for: scaffolding a new menu management page from scratch (use page-creation instead), editing route config files / menu seed SQL / hard-coded menu arrays, or anything that does not flow through the in-app 菜单管理 form.
---

# 菜单 / 功能 管理（操作型 skill）

本 skill 只覆盖**通过运行中的应用的"菜单管理"页面**去新增 / 修改 / 删除 **菜单（type=0）** 或 **功能（type=1）** 的场景。它不负责重新搭建菜单管理页面，也不直接改源码里的路由表或菜单种子数据 —— 那些场景请改用 `page-creation` 或直接编辑文件。

## 硬性前置：先确认菜单管理页面存在

执行任何动作前**必须**先做这一步。没有就停。

1. 用 Glob 在 `src/pages/` 下找候选目录（按优先级）：
   - `**/Menu/index.tsx`、`**/menu/index.tsx`
   - `**/permit/Menu/**`、`**/system/Menu/**`、`**/sys/Menu/**`、`**/admin/Menu/**`
   - 路由配置里搜 `菜单管理` 关键字也行（Grep `菜单管理` --type tsx/ts）。
2. 找到后，打开该文件粗看一眼，确认它是 **菜单管理列表页**（通常会有"菜单名称 / 类型 / 图标 / 权限标识 / 路由地址"这几列里的至少三列，且对应表单包含一个 RADIO 在"菜单 / 功能"两种类型之间切换）。
3. 如果**完全找不到**这样一个页面，**立即停下来**告诉用户：本项目未发现菜单管理页面，无法通过此 skill 操作，请改用 `page-creation` 先建一个，或直接编辑路由/菜单源数据。不要自己猜路径、硬塞。

确认存在后，再按需启动 dev server，用 Playwright MCP 走 UI 流程（参考 `playwright-mcp-strategy` skill 决定要不要真的开浏览器；如果用户只是让你"填正确"而不是"点出来"，可以只产出表单填写建议）。

## 两类实体的字段差异（核心规则）

菜单管理表单里**类型字段（通常 `type`）** 决定填什么。两类各自的必填和可选项不同。**不要把功能的字段填到菜单上，也不要反过来。**

### 菜单（type=0）

**用途**：在左侧导航 / 顶栏出现的可点击的可视节点（一级菜单 / 子菜单 / 叶子页面）。

**必须项**（除项目自定义额外字段外，至少包含）：
- 上级菜单（`pid`）—— 一级菜单留空 / 选根；其余必填。
- 菜单名称（`nm`）
- 类型（`type`）—— 选"菜单"。
- 路由地址（`path`）
- 显示排序（`seq`）

**额外必填（菜单特有）**：
- **图标（`icon`）** —— 菜单必须有图标，用项目的 ICONSELECT 选；不要留空也不要随便填字符串。

**菜单**通常还会有：分类（`cat`）、组件地址（`url`，叶子菜单要填）、显示状态（`status`，默认显示）。

**菜单不要填**：
- **权限标识（`perm`）** —— 菜单本身不承载权限码，留空。权限码只挂在该菜单下的"功能"上。如果表单上 perm 看起来是"必填红星"，那是表单组件的通用红星，不是这里的语义必填；菜单确实留空。

### 功能（type=1）

**用途**：不出现在导航里、只用来承载权限标识 `perm` 的"虚拟节点"。每一条 `perm`（例如 `sys:user:add`、`sys:user:upd`）对应后端接口的权限校验和前端按钮的 `hasPermi` 控制。

**必须项**：
- 上级菜单（`pid`）—— 一般挂在对应的业务菜单下（例如"用户管理"菜单下挂 用户管理的增删改查功能）。
- 类型（`type`）—— 选"功能"。
- 权限标识（`perm`）—— **必填**，详见下面两种模式。
- 显示排序（`seq`）

**功能不需要**：图标、路由地址、组件地址、显示状态 —— 这些是给真菜单看的导航字段，功能节点用不上，留空。即使表单上还显示这些输入框，也别填。

### 功能的两种添加模式（关键）

功能新增时**优先**用"基础功能批量勾选"模式，能勾就别一条条加。

#### 模式 A：基础功能批量（推荐，新增时用）

适用于：给某个业务菜单加上一整套 CRUD / 通用 RBAC 操作（查看、新增、修改、删除、导入、导出、审核 中的多个）。

操作：
1. 勾选"基础功能"那一栏的复选框（页面通常用一个 `CHECKBOXGROUP`，选项类似：查看 / 新增 / 修改 / 删除 / 导出 / 导入 / 审核）。
2. 在"权限标识"输入框里**只填前缀**——格式 `模块:功能`，例如 `sys:user`、`sys:rsco`、`biz:order`。
3. 菜单名称（`nm`）可以留空或随便填一个占位 —— 表单保存时会按"基础功能"的每个勾选项自动拼成多条记录，每条的 `nm` 用勾选项的中文标签（"新增" / "修改" …），每条的 `perm` 用 `前缀:操作 code`（`sys:user:add`、`sys:user:upd` …）。
4. 排序（`seq`）填一个起点（例如 `10`），保存逻辑会自动给后续项加偏移（10、20、30 …）。

**注意事项**：
- "查看"那一项在大多数项目里是个**多 code**的复合（典型值 `page,info,list,query`），保存时会被拆成 `前缀:page`、`前缀:info`、`前缀:list`、`前缀:query` 四条 perm 一并写入。不要被它的多 code 吓到，这是正常的。
- 此模式只在**新增**时可用；**编辑**时不要再去勾基础功能（编辑模式下该字段一般会被隐藏），编辑请用模式 B。

#### 模式 B：非基础功能 / 自定义权限（直接写完整 perm）

适用于：
- 业务自定义动作（"导出 PDF"、"批量审批"、"重置密钥"、"刷新缓存"等不在通用 CRUD 集合里的）。
- 任何**编辑现有功能记录**的场景。

操作：
1. **不要**勾选"基础功能"复选框（新增时让它保持空；编辑时该字段一般不可见）。
2. 菜单名称（`nm`）必填，写中文动作名（例如"导出 PDF"、"刷新缓存"）。
3. 权限标识（`perm`）填**完整**的三段式 `模块:功能:操作`，例如 `sys:user:exportPdf`、`sys:cache:refresh`。不能只填前缀。
4. 排序（`seq`）按上下文排。

## 一些保存前的二次检查

提交前过一遍下面这些，能避免 80% 的返工：

1. **类型选对了吗？** 切到"菜单"还是"功能"，导致表单可见字段会变。错选会把图标 / 路由填到了功能上，或把权限标识填到菜单上。
2. **菜单留 perm 空了吗？** 菜单条目的 perm 应当为空（表单组件可能仍允许输入，但语义上不要填）。
3. **功能有 perm 吗？** 功能的 perm 必填。
4. **基础功能模式下 perm 是不是只填了前缀？** 不要写成完整三段，否则保存逻辑会拼成 `sys:user:add:add` 这种重复。
5. **编辑模式没勾基础功能吧？** 编辑不会走批量拆分逻辑，勾了也不生效。
6. **`url` 是 .tsx 路径吗？** 项目里常见的保存逻辑会自动把以 `.tsx` 结尾的 `url` 截掉扩展名并把 `\` 转 `/`，所以填带扩展名也行，但更稳的是直接填不带扩展名的相对路径。
7. **图标只挑菜单填**：功能节点别填图标，导航里不会展示。

## 提交后的验证

通过 UI 操作完成后，让用户做这两件事之一来确认生效：
- 在菜单管理列表里刷新，确认新增的节点出现在预期的 `pid` 下、`type` 和 `perm` 正确。
- 如果项目有"刷新菜单缓存 / 刷新权限缓存"的按钮（菜单管理页常见），点一下，再让对应用户重新登录或刷新页面，确认新菜单出现 / 新权限生效。

## **必做的下一步：给超级管理员配上新菜单 / 新功能权限**

菜单 / 功能在菜单管理页保存只是把节点写进了"权限资源表"。**没有任何角色被自动授权**，包括超级管理员 —— 这就是为什么很多人新加完菜单刷新发现"我自己都看不到"。所以**每次新增**完菜单或功能，本 skill 都要继续走下面这步，不能漏：

### 1. 找到角色管理页面

按优先级 Glob 候选目录：
- `**/Role/index.tsx`、`**/role/index.tsx`
- `**/permit/Role/**`、`**/system/Role/**`、`**/sys/Role/**`、`**/admin/Role/**`
- 或者在路由里搜 `角色管理`（Grep）。

如果项目根本没有角色管理页面 —— 告诉用户："新菜单已建好，但本项目没有角色管理页面，需要后端 / DBA 手动把这些 perm 挂到超级管理员角色上"，然后停。

### 2. 进入"角色权限"分配弹窗

通常每行角色都有一个 **「角色权限」** 行内操作（按钮 / Operate 菜单项）。点击 **超级管理员**（常见名称："超级管理员"、"系统管理员"、"超管"、"admin"；常见 cd："admin"、"superadmin"；项目里有时还会用 `type=1` 标识系统内置不可删的超管角色）那一行的「角色权限」。

弹出的弹窗一般是一个**双树**结构（参考典型实现，例如本项目 `permit/Role/MenuAssign`）：
- 左侧：菜单树（仅菜单节点，type=0）
- 右侧：选中左侧某个菜单后，显示该菜单下挂的功能节点（type=1）
- 弹窗顶部一般会有提示语，类似："点击菜单查看对应功能，勾选菜单和功能进行权限分配"

### 3. 勾选这次新增的节点

- **新增了菜单**：在左侧菜单树里**勾选这个菜单**（如果有父菜单，确保父菜单也保持勾选状态 —— 树组件一般会半选 / 自动联动；如果半选不够，需要把父级一起补勾）。
- **新增了功能**：先在左侧菜单树**点击**（不是勾选，是选中查看）该功能所挂载的父菜单，然后在右侧功能树里勾上这次新增的那几个 perm。
- 模式 A（基础功能批量）下一次拆出来的多条 perm（查看 / 新增 / 修改 / 删除 …）**要全部勾上**，别只勾一条。

### 4. 保存

点弹窗的"确定 / 保存 / 提交"。保存后通常还需要做下面任一确认：
- 弹窗关闭无报错。
- 如果用户当前登录的就是超级管理员，**重新登录或刷新页面**（前端权限缓存通常在登录时拉一次），新菜单 / 新按钮应当立刻可见。
- 如果项目有「刷新权限缓存 / 刷新菜单缓存」按钮（常见于菜单管理或角色管理页顶栏），点一下再刷新。

### 5. 多角色的情况

用户如果只让"配给超级管理员"，那就只动超管这一行，**不要主动**给其他角色（普通用户 / 业务角色 / 只读角色）也勾上 —— 那是权限决策，应该交给用户明确指示。

### 6. 反向：删除了菜单 / 功能怎么办

删除菜单 / 功能节点时，通常后端会级联清理 `role_rt_rsco`（角色-资源关联表）；不需要再去角色管理里手工取消勾选。但保险起见，删完后让用户进一次超管的「角色权限」弹窗扫一眼有没有残留 / 报错。

## 不在本 skill 范围

下面这些场景**不要**用本 skill，请走别的：

- 项目里**没有**菜单管理页面 —— 让用户改用 `page-creation` 先建一个，或直接编辑路由源数据；本 skill 立即停止。
- 改前端路由配置文件（`src/router/**`、`src/routes/**`）—— 那是源码层的菜单，不走表单。
- 改后端菜单种子 SQL / 初始化脚本 —— 让用户去后端项目处理。
- 给页面里的按钮加 `hasPermi=[...]` 数组 —— 那是 page-creation / 页面编辑场景。

## 与其他 skill 的协作

- 如果**真的要去浏览器里点出这些操作**（不是只给字段建议），先调起 `playwright-mcp-strategy` 决定是不是真开浏览器 + 怎么登陆（`browser-auto-login-skill`）。
- 如果发现项目的菜单管理页面本身缺字段 / 行为不对，那是页面层 bug，先调起 `page-creation` 修页面，再回到本 skill 操作。
