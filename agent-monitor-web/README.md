# Vita Admin Starter

一套开箱即用的**中后台管理脚手架**。
UI 主要基于 [`@hsu-react/ui`](https://www.npmjs.com/package/@hsu-react/ui) 组件库构建，
配合 React 18 + TypeScript + MobX，使用 Vite 8 打包。
（`@hsu-react/ui` 在 Ant Design 5 之上做了二次封装，antd 作为底层依赖少量场景直接使用。）

内置动态路由（后端菜单驱动）、权限控制、多标签页、主题切换、国际化、服务层封装、
MobX store 基类（列表 / 表单 / CRUD）、通用组件库，以及页面脚手架生成器 —— 可直接作为新项目的起点。

> 内置系统管理示例页（权限、日志、字典、文件等），方便在此之上叠加自己的业务。

## 技术栈

| 分类 | 选型 |
| --- | --- |
| 框架 | React 18、React Router 6 |
| 语言 | TypeScript 5 |
| 状态管理 | MobX 6 + mobx-react-lite |
| UI 组件库（主） | **`@hsu-react/ui`** —— Panel / Form / Table / Search / FormItem / Operate / Chart / Markdown 等业务组件，页面主要由它搭建 |
| UI 底层（次） | Ant Design 5 —— `@hsu-react/ui` 的底层依赖；少量基础元素（Button / Tag / Switch 等）按需直接使用 |
| 可视化 | ECharts、AntV G6、ReactFlow、zrender |
| 富文本 / 编辑器 | TipTap、wangEditor、react-markdown、CodeMirror |
| 表格 | x-data-spreadsheet、xlsx |
| 网络 | axios、@microsoft/fetch-event-source（SSE 流式） |
| 构建 | Vite 8、@vitejs/plugin-react、sass、terser |
| 加密 | crypto-js、node-forge、@noble/ciphers（登录 RSA + AES） |

路径别名：`@/*` → `src/*`（见 `tsconfig.json` 与 `vite.config.ts`）。
组件统一**直接从 `@hsu-react/ui` 引入**。

## UI 组件库

项目页面**主要由 [`@hsu-react/ui`](https://www.npmjs.com/package/@hsu-react/ui) 搭建**——
列表页 `Panel.List`、内容 / 表单页 `Panel.Default` / `Form.Modal` / `FormItem`、
搜索 `Search`、操作列 `Operate`、表格 `Table`、图表 `Chart`、`Markdown` 等业务组件均来自它
（已基于 Ant Design 5 做二次封装并统一观感）。组件均直接从 `@hsu-react/ui` 引入：

```tsx
import { Panel } from "@hsu-react/ui";
import { FormItemProps } from "@hsu-react/ui";
import { Operate } from "@hsu-react/ui";
```

**Ant Design 5 作为底层依赖**，只在需要原生基础元素时按需直接使用：

```tsx
import { Button, Tag, Switch, message } from "antd";
```

> 组件文档与在线示例见 hsu-ui 官网：<https://vitatsui.github.io/hsu-ui>
> 组件能力的改动需回到 hsu-ui 仓库，发布新版本后再升级本项目的依赖。

## 环境要求

- Node.js 20+
- 包管理器：yarn（推荐）

## 快速开始

```bash
# 1. 安装依赖
yarn

# 2. 启动开发服务器
yarn start
```

默认监听 **3003** 端口（被占用时由 portfinder 自动顺延，终端会打印实际地址）。
启动前先 `cp .env/.env.dev.example .env/.env.dev`，把 `API_PROXY` 指向你的后端，
并填好登录加密密钥（`.env.dev` / `.env.prod` 带具体部署值，不入库）。

## 常用脚本

| 命令 | 说明 |
| --- | --- |
| `yarn start` | 启动开发服务器（Vite，HMR） |
| `yarn build` | 生产构建（先 `tsc --noEmit` 过类型，再 `vite build` 输出到 `dist/`） |
| `yarn lint` | ESLint 检查（`--max-warnings 0`） |
| `yarn crt:lp` | 生成列表页面（List Panel） |
| `yarn crt:fp` | 生成表单页面（Form Panel） |
| `yarn crt:lfp` | 生成「列表 + 表单」页面 |
| `yarn crt:lmp` | 生成「列表内嵌弹窗子列表」页面 |
| `yarn crt:dp` | 生成静态 / 内容页面（Default Panel） |

> `crt:*` 为页面脚手架，脚本在 `scripts/`，对应模板在 `scripts/{ListPanel,FormPanel,...}`。
> 这是本框架快速产出新页面的核心利器，强烈建议先熟悉。

## 环境变量

环境配置集中在 `.env/` 目录，由 `vite.config.ts` 用 dotenv 读出来、在构建时 `define` 成 `process.env.X` 内联进产物：

| 文件 | 用途 |
| --- | --- |
| `.env.common` | 公共配置（如 CSS Modules 类名放行白名单 `PASS_CLS`）。**入库** |
| `.env.dev.example` / `.env.prod.example` | 模板，只有结构与占位符。**入库** |
| `.env.dev` | 开发环境：端口、`/api` 代理目标、登录加密密钥。**不入库**，`cp` 模板后自己填 |
| `.env.prod` | 生产环境。**不入库**，由 `bash scripts/write-env-prod.sh` 从模板生成 |

`.env.dev` / `.env.prod` 里的 `CRYPTO_KEY`、`RSA_PUB_KEY` 必须与对应 hub 的
`AM_CRYPTO_KEY` / RSA 私钥配对，是「部署方持有的配置」，所以这两个文件不进版本库。
生产构建：

```bash
CRYPTO_KEY=… RSA_PUB_KEY=… bash scripts/write-env-prod.sh && yarn build
```

CI 里这两个值取自仓库变量 `WEB_CRYPTO_KEY` / `WEB_RSA_PUB_KEY`。

`.env.dev` 关键项：

- `SERVER_PROT` — dev server 端口（3003，被占用会自动顺延）
- `API_BASE` / `API_PROXY` — `/api` 反向代理到你的后端
- `CRYPTO_KEY` / `RSA_PUB_KEY` — 登录加密密钥，需与后端配置一一对应（**请替换为你自己的密钥**）

## 目录结构

```
vita-admin-starter/
├─ .env/                 # 环境变量（common / dev / prod）
├─ vite.config.ts        # Vite 配置（env 注入 / CSS Modules 类名 / 代理 / 分包）
├─ scripts/              # 页面脚手架生成器（crt:* 对应脚本与模板）
├─ public/               # 静态资源（config.js 含站点标题等运行时配置）
└─ src/
   ├─ assets/            # 图片 / 字体 / lottie 等
   ├─ hooks/             # 通用 hooks（权限 / 搜索 / 标签页等）
   ├─ layout/            # 布局（菜单 / 面包屑 / 标签栏 / 主题 / 国际化）
   ├─ pages/             # 页面（见下方模块说明）
   ├─ router/            # 动态路由配置、菜单驱动、路由守卫
   ├─ services/          # Axios 封装、接口定义（apis/）、返回类型
   ├─ stores/            # MobX store（OptionsStore、基类 basisStoreClass）
   ├─ styles/            # 全局样式
   └─ utils/             # 工具函数（auth / crypto / wsCache …）
```

### 页面模块（系统管理示例）

| 目录 | 模块 |
| --- | --- |
| `pages/Login`、`pages/PwdChange` | 登录、修改密码 |
| `pages/Overview` | 概览 / 首页（静态示例仪表盘，接入接口即可） |
| `pages/permit` | 权限：部门 Dept、菜单 Menu、角色 Role、用户 User |
| `pages/syslog` | 系统日志：API / 错误 / 任务 / 登录 / 操作 等日志 |
| `pages/sysmgmt` | 系统管理：字典 Dict、参数 Param、短信 sms |
| `pages/systool` | 系统工具：文件 File |

> 这些页面同时是「如何用本框架写业务页」的范例：列表 + 搜索 + 表单弹窗 + CRUD 的标准写法，
> 配合 `scripts/` 的脚手架生成器即可快速复制扩展。

## 路由说明

- 后台统一前缀 `/admin`，菜单由后端动态返回并挂载为子路由（见 `src/router/RouterService.tsx`）。
- 带路由参数、不在菜单中的详情页，在 `src/router/router.config.tsx` 中显式注册。
- 用 `adminPath("xxx/yyy")` 拼接后台路径，避免硬编码 `/admin` 前缀。

## 测试账号

- 账号: admin
- 密码: admin123
