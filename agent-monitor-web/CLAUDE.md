# 项目规范（每次会话必读）

## UI 组件选用优先级（硬性规则）

1. **一律优先从 `@hsu-react/ui` 导入**：`Button`、`Icon`、`Table`、`FormItem`、`Form`（含 `Form.Modal`）、`Panel`（`List`/`Default`/`List.Modal`）、`Search`、`Operate`、`Switch`、`Select`、`Input`、`Checkbox`、`DatePicker`、`Tree`、`Tags`、`Modal`、`Descriptions`、`TextEllipsis`、`Copy`、`Upload`、`TabBar`、`Slider`、`FlexFill`、`Chart`、`Markdown`、`CodeMirror`、`Editor`、`FilePreview`、`Spreadsheet`、`ChainGraph`、`Chat` 等。
2. hsu-ui 没有的能力才用 antd 兜底（如 `message`、`notification`、`Popover`、`Tooltip`、`Divider`、`Segmented`、`Spin`、`Empty`）。
3. **禁止**用 antd 同名组件替代 hsu-ui 已有组件（antd 的 `Table`/`Modal`/`Switch`/`Select`/`Button` 等都不允许直接用）。拿不准先查 `node_modules/@hsu-react/ui/es/index.d.ts` 的导出或文档站 <https://vitatsui.github.io/hsu-ui>。

组件能力不满足需求时，改动回 hsu-ui 仓库发版，再升级本项目依赖；不要在本项目内 fork/覆写组件。

## 页面 / 接口 / 选项 / 菜单开发

创建或修改相应内容时，先调起 `.claude/skills/` 下的项目级 skill 并遵循其规范：

- `page-creation`：`src/pages/` 下任何页面的创建/修改/审查
- `api-creation`：`src/services/apis/` 下接口模块
- `options-management`：`OptionsStore` 下拉/枚举选项
- `menu-function-management`：运行中应用的菜单/功能管理
- `playwright-mcp-strategy`：前端源码改动与浏览器验证

## 技术栈约定

- React 18 + TypeScript + MobX + webpack 5
- 样式一律 scss（`.module.scss`），项目内零 less
- 列表页 store 继承 `ListPanelStore`、表单 store 继承 `FormModalStore`（`src/stores/basisStoreClass/`）
- 入口 `src/index.tsx` 引入组件库全局样式 `@hsu-react/ui/es/styles/antd-overload.scss`；项目特有的 antd 覆盖增量放本地 `src/styles/antd-overload.scss`（在其后引入），不要整份拷贝组件库样式
