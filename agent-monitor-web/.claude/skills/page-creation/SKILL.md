---
name: page-creation
description: Use this skill when creating, modifying, reviewing, auditing, or scaffolding any page/module under `src/pages/` in a React + MobX + Ant Design project that uses the project-local `Panel`/`Form.Modal`/`FormItem`/`Operate` component suite and the `ListPanelStore`/`FormModalStore` base classes (located under `src/stores/basisStoreClass/`). Covers five page shapes — CRUD lists (`Panel.List`), form modals (`Form.Modal`), list+form bundles, static/dashboard content pages (`Panel.Default`), and nested list-in-modal pages (`Panel.List.Modal`) with parent-ID scoping — plus the private sub-folders they spawn (`_components/`、`_contComps/`、`_hooks/`、`_utils/`、`<Module>Form/`). Also triggers when editing or adding to existing pages: 加/改/删列（含操作列、开关列、`fixedWidth`、`orderKey`）、加/改/删搜索项、加/改/删表单项、加/改按钮（含 `hasPermi` 权限码）、加/改弹窗、切换 Dsr 字段渲染、调整列宽档位、拆 `_contComps/`、在列表 store 里覆写 `_modeType`/`_staticSearchData`、把表单 store 的 `_getFormData` 接上、或为已有列表追加嵌套子列表。以及对已有页面做合规 review（会连带调起 api-creation 检查 `src/services/apis/` 配套文件）。Typical trigger phrases: "新增一个页面"、"创建一个 XXX 管理页面"、"add a CRUD page"、"在 pages 下新增模块"、"新建一个增删改查界面"、"新建一个仪表盘/内容页/静态面板"、"在 XXX 里弹一个子列表/嵌套列表"、"给 XXX 页面加列/加搜索项/加表单项/加按钮/加弹窗"、"改下 XXX 页面的列宽/权限码/操作列"、"把 XXX 列表的状态列换成 Dsr"、"检查这个页面是否符合规范"、"review / 审计 pages 下 XXX". The skill adapts to the current project's own directory/component conventions rather than assuming a fixed category list.
---

# 页面创建规范 (src/pages)

本 skill 规范 `src/pages/` 下 CRUD 业务页面的目录结构、Store 约定和组件装配方式。目标是产出与既有模块一致的代码，而不是自由发挥。

## 先决条件 — 先读这些

创建页面前必须确认以下基础设施（若缺失，此 skill 不适用）：

- `src/stores/basisStoreClass/ListPanelStore.ts` —— 列表页基类，暴露 `searchData`/`page`/`dataSource`/`total`/`isLoading`/`order` 等 observable，以及 `setSearchData`/`initSearchData`/`changePage`/`onOrderChange`/`getDataSource`/`delData`/`resetStore` 等方法。基类在排序/翻页/查询路径会先置空列表再调 `getDataSource`（避免展示旧数据）；另有 `getTotal`（条数独立请求，仅查询触发）与 `_patchRow`/`_refreshRowData`（按主键行级更新，不触发整表刷新）。
- `src/stores/basisStoreClass/FormModalStore.ts` —— 表单弹窗基类，暴露 `formData`、`getFormData`、`addFormData`、`editFormData`、`resetFormData`、`_message`。
- 配对好的服务层：`src/services/apis/<category>/<module>.ts`（必须镜像页面路径，详见 api-creation skill）。
- 组件套件（多数在 `src/components/` 下）：`Panel.List` / `Panel.Default`、`Form.Modal`、`FormItem`（含 `FormItemProps`）、`Table`（含 `ColumnsType`）、`Operate`、`Button`（含 `ChakraButtonProps`）、`Switch`。
- 可选：`src/stores/OptionsStore` 提供 `Options("KEY")` 选项工厂与按需加载方法（`getRole`、`getXxxType` 等）。**仅承载下拉/枚举选项**——非选项类的数据（系统参数、单值字符串、健康状态等）不进这里，详见后文「列表页 Store 模板」的要点。

## 与 api-creation skill 的联动（必须遵守）

本 skill 和 `api-creation` skill 是一对配套规范：页面和 API 在路径/文件名上必须镜像一致（详见 api-creation skill 的「目录和文件名」一节）。

**两类触发场景必须**通过 `Skill` 工具调起 `api-creation`：

1. **创建页面时**：若本次页面创建涉及新增或修改 `src/services/apis/` 下的任何文件（无论是全新模块还是在已有文件里加一两个函数），**在动 API 文件之前**先调用 `api-creation` skill 确认路径镜像、文件名、类型/函数命名与写法。不要凭记忆自己对着模板硬写 API 文件。
2. **审计/Review 既有页面时**：检查页面合规性时，**必须**连带对页面配套的 `src/services/apis/` 文件做一次 api-creation 合规检查——调起 `api-creation` skill 获取最新规则后再给结论。页面规范本身不能覆盖 API 层的问题。

如果能一次就确定既要创建页面又要创建 API，**并行**调起两个 skill（本 skill + api-creation）；不要先只按页面规范落笔再回头补 API 规范，那样会反复修改。

## 目录骨架

一个标准 CRUD 模块的目录：

```
src/pages/<category>/<ModuleName>/
├── index.tsx                    # 列表页入口（observer）
├── index.module.less            # 模块级样式（通常只占位，.ModuleName {}）
├── <ModuleName>Store.ts         # 列表页 store，extends ListPanelStore
└── <ModuleName>Form/            # 新增/编辑弹窗（如需表单）
    ├── index.tsx
    ├── index.module.less
    └── <ModuleName>FormStore.ts # 表单 store，extends FormModalStore
```

若模块还需要额外的子弹窗/子页面（如"数据"、"批量操作"、"详情"），与 `<ModuleName>Form/` 平级再开一个 PascalCase 文件夹（例如 `DataModal/`、`BatchRoleManagement/`、`FieldDetailList/`），结构同上。

### 大类目录（category）

大类目录由**当前项目**实际存在的子目录决定。创建前先用 Glob 列出 `src/pages/` 的子目录，择一归入；都不匹配再新建。独立入口（登录、首页、改密等）直接放在 `src/pages/` 根下。

### 页面私有资源用下划线前缀

页面内部不对外共享的构件用下划线前缀目录承载，表示"仅本页使用"：

- `_components/`：页面私有的展示组件
- `_contComps/`：页面内容区（content components）拆分
- `_hooks/`：页面私有 hooks
- `_utils/`：页面私有工具函数
- `_constants/`：页面私有常量
- `_styles/`、`_context/`、`_modals/` 等按需

不要把页面私有代码抽到 `src/components/`——那是全项目共享级别。

## 命名

- **文件夹/模块名**：PascalCase，与业务名一一对应，例如 `ApiBlacklistManagement`、`MqQueueManagement`、`Dict`。
- **Store 文件**：`<ModuleName>Store.ts`、`<ModuleName>FormStore.ts`。
- **样式文件**：固定 `index.module.less`，外层类名用 PascalCase 与文件夹同名（`.ApiBlacklistManagement { }`），内部类名 camelCase。
- **LESS 类命名**：`styles.ApiBlacklistManagement`（外层）、`styles.content`、`styles.formItemGroupTitle`（内层 camelCase）。
- **props 类型**：`<ModuleName>FormProps`，放在组件文件顶部，不单独抽文件。

## 组件选用：@hsu-react/ui 优先（硬性规则）

页面里用到的 UI 组件**一律优先从 `@hsu-react/ui` 导入**，antd 只兜底 hsu-ui 没有的能力——不要凭习惯从 antd 拿：

- **hsu-ui 已覆盖，必须用 hsu-ui 版**：`Button`、`Icon`、`Table`、`FormItem`、`Form`（含 `Form.Modal`）、`Panel`、`Search`、`Operate`、`Switch`、`Select`、`Input`、`Checkbox`、`DatePicker`、`Tree`、`Tags`、`Modal`、`Descriptions`、`TextEllipsis`、`Copy`、`Upload`、`TabBar`、`Slider`、`FlexFill`、`Chart`、`Markdown`、`CodeMirror`、`Editor`、`FilePreview`、`Spreadsheet`、`ChainGraph`、`Chat`、`SecondConf`、`FileCol`。
- **antd 兜底（hsu-ui 无对应）**：`message`、`notification`、`Popover`、`Tooltip`、`Divider`、`Segmented`、`Spin`、`Empty`、`Row/Col` 栅格、`Space` 等。
- 拿不准某组件 hsu-ui 有没有：先查 `node_modules/@hsu-react/ui/es/index.d.ts` 的导出（或文档站 <https://vitatsui.github.io/hsu-ui>），再决定 import 来源。

反例：

```tsx
// ❌ hsu-ui 已有同名组件却从 antd 导入 —— 观感与行为都会偏离项目规范
import { Table, Modal, Switch, Button } from "antd";
```

```tsx
// ✅ 同名组件一律走 hsu-ui，antd 只补缺
import { Table, Modal, Switch, Button } from "@hsu-react/ui";
import { message, Tooltip } from "antd";
```

## 列表页 `index.tsx` 模板

```tsx
import React, { useEffect, useState } from "react";

import { ChakraButtonProps, ColumnsType, FormItemProps, Panel, Operate } from "@hsu-react/ui";
import { PlusOutlined } from "@ant-design/icons";
import { observer } from "mobx-react-lite";

import XxxStore from "./XxxStore";
import XxxForm from "./XxxForm";
import XxxFormStore from "./XxxForm/XxxFormStore";
import styles from "./index.module.less";

const Xxx: React.FC = observer(() => {
  const {
    setSearchData,
    initSearchData,
    dataSource,
    isLoading,
    total,
    changePage,
    page,
    getDataSource,
    delData,
    order,
    onOrderChange,
  } = XxxStore;
  const [id, setId] = useState<number | string>();
  const [open, setOpen] = useState(false);
  const [title, setTitle] = useState("");

  useEffect(() => {
    initSearchData();
  }, [initSearchData]);

  const searchItems: FormItemProps[] = [
    { type: "INPUT", name: "nm", label: "名称" },
  ];

  const beforeButtonGroup: ChakraButtonProps[] = [
    {
      title: "新增",
      colorPalette: "blue",
      icon: <PlusOutlined />,
      onClick: () => {
        setTitle("新增");
        setId("");
        setOpen(true);
      },
      hasPermi: ["<module>:<sub>:add"],
    },
  ];

  const columns: ColumnsType = [
    { title: "名称", dataIndex: "nm", width: 200 },
    {
      title: "操作",
      width: 140,
      ellipsis: false,
      align: "center",
      fixed: "right",
      fixedWidth: true,
      render: (record) => (
        <Operate
          menu={[
            {
              title: "编辑",
              onClick: () => {
                setTitle("修改");
                setOpen(true);
                setId(record.id);
              },
              hasPermi: ["<module>:<sub>:upd"],
            },
            {
              title: "删除",
              delete: true,
              onConfirm: () => {
                delData(record.id);
              },
              hasPermi: ["<module>:<sub>:del"],
            },
          ]}
        />
      ),
    },
  ];

  return (
    <>
      <Panel.List
        className={styles.Xxx}
        searchProps={{
          searchItems,
          onSearch: setSearchData,
          onReset: initSearchData,
          beforeButtonGroup,
          hasPermi: ["<module>:<sub>:query"],
        }}
        tableProps={{
          columns,
          dataSource,
          rowKey: "id",
          loading: isLoading,
          pagination: {
            total,
            onChange: (num, size) => changePage({ num, size }),
            current: page?.num,
            pageSize: page?.size,
          },
          order,
          onOrderChange,
        }}
      />
      <XxxForm
        open={open}
        title={title}
        id={id}
        onCancel={() => {
          setId("");
          setOpen(false);
        }}
        onOk={() => {
          getDataSource();
        }}
      />
    </>
  );
});

export default Xxx;
```

要点：
- 默认导出 `observer(() => {...})` 包裹的组件。
- 从 store 解构所需 observable 与 action，变量名不要改。
- 用 `useState` 维护弹窗本地 UI 状态（`id`、`open`、`title`）——**不要**把这类短命 UI 态塞进 MobX store。
- `useEffect(initSearchData, [initSearchData])` 是入口触发数据加载的标准写法。
- 枚举选项在组件挂载时通过 `OptionsStore` 的对应方法加载（`useEffect(() => getXxxType(), [getXxxType])`）。
- 所有按钮/列/操作项都要带 `hasPermi: [<权限码>]` 做权限门控，权限码格式由后端决定（如 `vm:monitApiBlcak:add`）。

## 列表页 Store 模板

```ts
import {
  XxxData,
  XxxSearchData,
  deleteXxx,
  getXxxList,
} from "@/services/apis/<category>/<module>";

import ListPanelStore from "@/stores/basisStoreClass/ListPanelStore";
import { makeObservable } from "mobx";

class XxxStore extends ListPanelStore<XxxSearchData, XxxData> {
  protected accessor _modeType = {};

  constructor() {
    super();
    makeObservable(this);
  }

  public getDataSource = () => {
    getXxxList({ query: this._query.value })
      .then((res) => {
        if (res.code === 0) {
          const { list, page } = res.data;
          const { total } = page;

          this._dataSource = list;
          this._total = total;
        } else {
          this._message(res);
        }

        this._isLoading = false;
      })
      .catch(() => {
        this._isLoading = false;
      });
  };

  public delData = (id: number | string) => {
    deleteXxx(id).then((res) => {
      if (res.code === 0) {
        this.getDataSource();
      }

      this._message(res);
    });
  };
}

export default new XxxStore();
```

要点：
- **导出单例**：`export default new XxxStore();`，页面直接 `import XxxStore from "./XxxStore"` 拿到同一个实例。
- 列表获取覆写 `public getDataSource`；排序/翻页/查询路径由基类先置空列表再调用（防旧数据闪现），`delData` 成功后的刷新不置空（无闪烁）。状态开关等单行变更优先用 `_refreshRowData(id, 列表接口)` 行级刷新（内部防乱序、未查到自动回退整表），不要整表重拉。
- `_isLoading = false` 必须在 `.then` 和 `.catch` 两条路径都设置，避免卡 loading。
- 成功分支只在 `res.code === 0` 下处理数据；失败分支统一走 `this._message(res)`。
- **`_modeType` 始终声明**（即便是 `{}`）——脚手架和现有模块都显式挂了 `protected accessor _modeType = {};`，子类保持一致，便于后面按需加模式而不改签名。
- 若有"固定不变的查询条件"（如 `tid: 0`），覆写 `_staticSearchData`；字段需要特殊比较模式（如 `IS`、`EQ`）时在 `_modeType` 里登记，例如嵌套列表按父级 ID 精确过滤：`protected accessor _modeType = { excepStratId: "EQ" };`。
- `delData` 成功后再次调用 `getDataSource()` 刷新；不要自行 splice `_dataSource`。
- **系统参数（`/sys/param/getValByKey`）等非选项类数据放页面自己的 Store，不要塞进 `OptionsStore`**：`OptionsStore` 只承载下拉/枚举选项（`Options("XXX")` 工厂消费的那种 `SelectOption[]`）。需要从 `/sys/param/getValByKey` 取的 system param、健康检查值、单值字符串等，挂在使用该值的页面 Store 上，按 `@computed get xxx() / @observable accessor _xxx / public getXxx = () => apiCall().then(...)` 的三段式落地（参考 `DataViewSearchStore` 的 `defaultColumnCount`/`getDefaultColumnCount`）。多个页面需要同一个系统参数也照此办理：每个页面 Store 各放一份，让数据归属和加载时机跟着页面走，而不是堆到全局 store。判断标准：如果这个值不会通过 `Options("KEY")` 当成下拉项渲染，就不属于 `OptionsStore`。

## 表单弹窗 `XxxForm/index.tsx` 模板

```tsx
import React, { useEffect } from "react";

import { Form, FormItemProps } from "@hsu-react/ui";
import { observer } from "mobx-react-lite";

import XxxFormStore from "./XxxFormStore";
import styles from "./index.module.less";

interface XxxFormProps {
  open?: boolean;
  title?: string;
  id?: string | number;
  onCancel?: () => void;
  onOk?: () => void;
}

const XxxForm: React.FC<XxxFormProps> = observer((props) => {
  const { open, title, id, onCancel, onOk } = props;
  const { resetFormData, addFormData, editFormData, formData, getFormData } =
    XxxFormStore;

  useEffect(() => {
    if (id && open) {
      getFormData(id);
    }
  }, [getFormData, id, open]);

  const formItems: FormItemProps[] = [
    { type: "INPUT", name: "nm", label: "名称", required: true },
    // ...
  ];

  const onClose = () => {
    resetFormData();
    onCancel?.();
  };

  const handleOk = (data: Record<string, unknown>) => {
    if (id) {
      editFormData(id, data, () => {
        onClose();
        onOk?.();
      });
    } else {
      addFormData(data, () => {
        onClose();
        onOk?.();
      });
    }
  };

  return (
    <Form.Modal
      className={styles.XxxForm}
      title={title}
      open={open}
      onCancel={onClose}
      onOk={handleOk}
      formItems={formItems}
      value={formData}
    />
  );
});

export default XxxForm;
```

要点：
- Props 固定四件套：`open`、`title`、`id`、`onCancel`、`onOk`（可加 `dataId`、`data` 等业务补充字段）。
- `useEffect` 中 `if (id && open)` 双重条件，避免弹窗未打开时空请求。
- 依赖 `OptionsStore` 的字段，放到**另一个** `useEffect(() => getXxxType(), [getXxxType, open])`——按需拉一次。
- **把 `onClose` 和 `handleOk` 提成命名常量**，不要在 JSX 里写多层内联箭头函数；`Form.Modal` 的 `onOk={handleOk}`、`onCancel={onClose}` 只接引用。`handleOk` 的 `data` 形参统一标 `Record<string, unknown>`。
- 提交入口在 `handleOk` 里分支：有 `id` 走 `editFormData`，否则 `addFormData`；成功回调里 `onClose()` 再调 `onOk?.()` 通知父组件刷新。
- `onCancel` 关闭前一定调用 `resetFormData()`，否则下次打开新增时会带脏数据。
- `formItems` 字段多时可分组：`Record<string, FormItemProps[]>`，`Form.Modal` 会按分组渲染。
- 复杂嵌套表（动态行/子表）使用 `antd` 的 `Form.List` + 自定义 `Table` 渲染，详见 `ApiMetadataManagement/ApiMetadataManagementForm` 的实现。
- **父级 ID 注入**：当表单由外层父列表弹出（见下文"嵌套列表弹窗"一节）、需要把父记录 ID 一并写入提交数据时，props 里加 `<parentIdProp>?: number | string;`，解构出来后在 `handleOk` 开头写 `data.<parentIdProp> = <parentIdProp>;` 再走 `addFormData/editFormData`。

## 条件字段：基于其他字段值动态显示/必填

某些字段需要根据另一个字段的当前值决定**是否显示**、**是否必填**（例如"规则类型"决定要不要填"用户/系统类型/接口URL"）。实现按以下规则做：

1. **本地 `useState` 持有触发字段的当前值**（不要塞进 store）。`useEffect` 监听 `formData.<触发字段>` 同步给 state，覆盖编辑回填场景。

2. **用触发字段自身的 `componentProps.onChange` 更新 state**——**不要**在 `Form.Modal` 上挂 `onValuesChange` 监听全表单。`onValuesChange` 是"全局监听器"，要在回调里 if-else 挑字段，写法冗余、依赖排查难；字段自己的 `onChange` 只关心自己的值，本地 state 立即收敛。

3. **条件字段同步设 `visible` 和 `required`**，二者都基于派生 boolean。`visible: false` 会让 FormItem 直接 `return null`，不渲染；`required: false` 自动放过校验。隐藏后 antd Form 字段的缓存值仍会随提交一起发出（`preserve: true` 是默认），如果业务上不能多带这些值，要在 `componentProps.onChange` 切换触发字段时显式 `form.setFieldsValue({ <隐藏字段>: undefined })`。

4. **DATEPICKER 自身已经按 `showTime` 序列化好字符串**（`"YYYY-MM-DD"` 或 `"YYYY-MM-DD HH:mm:ss"`），`handleOk` 里**不要再额外格式化**。

```tsx
const [ruleType, setRuleType] = useState<string | undefined>();

useEffect(() => {
  if (formData.ruleType) setRuleType(formData.ruleType as string);
}, [formData.ruleType]);

const needUser = ruleType === "USER_TOTAL" || ruleType === "USER_SYSTEM_TOTAL";
const needUrl = ruleType === "API_USER_TOTAL";

const formItems: FormItemProps[] = [
  {
    type: "SELECT",
    name: "ruleType",
    label: "规则类型",
    required: true,
    componentProps: {
      options: Options("RULE_TYPE"),
      onChange: (value: string) => setRuleType(value),   // ← 字段自身 onChange
    },
  },
  {
    type: "SELECT",
    name: "userId",
    label: "用户",
    required: needUser,
    visible: needUser,    // ← 隐藏即 return null
    componentProps: { options: Options("USER") },
  },
  {
    type: "SELECT",
    name: "url",
    label: "接口URL",
    required: needUrl,
    visible: needUrl,
    componentProps: { options: Options("API_METADATA") },
  },
];
```

反例：

```tsx
// ❌ 用 Form.Modal 的 onValuesChange 来监听单个字段 —— 维度错了
<Form.Modal
  onValuesChange={(value) => {
    if ('ruleType' in value) setRuleType(value.ruleType);  // 一堆 if-else
    if ('xxx' in value) ...
  }}
  ...
/>

// ❌ 提交前手动把隐藏字段抹 undefined —— 用 visible + onChange 时清就够了
const handleOk = (data) => {
  if (!needUser) data.userId = undefined;
  if (!needUrl) data.url = undefined;
  ...
};

// ❌ 自己 dayjs 再 format 一遍 DatePicker 的输出 —— 组件早格式化好了
const submitData = {
  ...data,
  rangeStartTm: dayjs(data.rangeStartTm).format("YYYY-MM-DD HH:mm:ss"),
};
```

## 表单 Store 模板

```ts
import {
  XxxData,
  createXxx,
  editXxx,
  getXxx,
} from "@/services/apis/<category>/<module>";

import FormModalStore from "@/stores/basisStoreClass/FormModalStore";
import { ResType } from "@/services/Axios";
import { makeObservable } from "mobx";

class XxxFormStore extends FormModalStore<XxxData> {
  constructor() {
    super();
    makeObservable(this);
  }

  protected _getFormData = (id: number | string) => {
    getXxx(id).then((res) => {
      if (res.code === 0) {
        this._formData = res.data;
      } else {
        this._message(res);
      }
    });
  };

  public addFormData = (data: XxxData, fn?: (res: ResType) => void) => {
    createXxx(data).then((res) => {
      if (res.code === 0) {
        fn?.(res);
      }

      this._message(res);
    });
  };

  public editFormData = (
    id: number | string,
    data: XxxData,
    fn?: (res: ResType) => void
  ) => {
    data.id = id;

    editXxx(data).then((res) => {
      fn?.(res);

      this._message(res);
    });
  };
}

export default new XxxFormStore();
```

要点：
- 详情方法**必须**用 `_getFormData`（protected 覆写），不要自己另开 `getFormData`——基类 `getFormData` 做了 `setTimeout(500)` 防抖。
- `addFormData`/`editFormData` 成功后调用传入的 `fn?.(res)`，再 `_message(res)`。
- 编辑提交前把 `id` 合并到 `data` 里（`data.id = id`）再 POST，和后端约定一致。

## Store 状态暴露：按是否继承基类分两种写法

不同继承情况下 store 的写法规则不同——这是 MobX 本身的限制：

- **继承 `ListPanelStore` / `FormModalStore` 的 store**：**必须** `makeObservable(this)` + 显式 `@observable accessor` + `@computed get`。`makeAutoObservable` 在继承链下会报错，基类本身用的也是 `makeObservable`。
- **没有任何继承的独立 store**：**必须** `makeAutoObservable(this)` + 裸私有字段 + 裸 getter，**不要写装饰器**。`makeAutoObservable` 会自动把成员推断成 observable/computed/action。

### 模式 A · 继承基类（`ListPanelStore` / `FormModalStore`）

基类里每一个公开字段都走「私有 observable accessor + computed getter」模式，子类新增字段**必须**沿用，不得裸字段赋值。

```ts
import { computed, makeObservable, observable } from "mobx";

class XxxStore extends FormModalStore<never> {
  constructor() {
    super();
    makeObservable(this);
  }

  // 1) 公开读取 —— @computed get
  @computed
  get roles() {
    return this._roles;
  }

  // 2) 私有可变状态 —— @observable protected/private accessor
  @observable
  private accessor _roles: RoleItem[] = [];

  // 3) 修改动作 —— 箭头函数字段（makeObservable 会自动识别为 action）
  public loadRoles = () => {
    getRoleList().then((res) => {
      if (res.code === 0) {
        this._roles = res.data?.list || [];
      } else {
        this._message(res);
      }
    });
  };
}

export default new XxxStore();
```

关键点：
- 必须 `makeObservable(this)`；继承链下 `makeAutoObservable` 会抛错。
- 私有字段一律 `@observable accessor`（`private`/`protected` 皆可，禁用裸字段）。
- 公开读取一律 `@computed get`，不要直接把私有字段 `public` 出去。
- 类型上有初始值就别标 `undefined`；必要时用 `| undefined` 并配 `= undefined`。

### 模式 B · 独立 store（无继承）

`Panel.Default` 仪表盘页、或任何不继承 `ListPanelStore`/`FormModalStore` 的独立 store。直接 `makeAutoObservable` + 裸私有字段 + 裸 getter，**不写任何装饰器**：

```ts
import { makeAutoObservable } from "mobx";

interface SearchData {
  timeType: number;
  date: string;
}

class XxxStore {
  private _searchData: SearchData = { timeType: 3, date: "" };
  private _list: RowItem[] = [];

  constructor() {
    makeAutoObservable(this);
  }

  get searchData() {
    return this._searchData;
  }

  get list() {
    return this._list;
  }

  public init = () => {
    this.fetchList();
  };

  public fetchList = async () => {
    const res = await getXxxList(this._searchData);
    if (res.code === 0) {
      this._list = res.data?.list ?? [];
    } else {
      showResError(res);
    }
  };
}

export default new XxxStore();
```

关键点：
- 只调用 `makeAutoObservable(this)`，不要 import `observable` / `computed` / `action`，也不要写 `@observable` / `@computed` / `accessor`。
- 私有字段保持 `_` 前缀 + 公开 getter 的只读边界，命名与模式 A 一致；只是少了装饰器。
- 业务 `code !== 0` 的错误提示用项目已有的 `showResError(res)`（`src/pages/cockpit/_utils/utils.ts`），不要直接 `notification.error`。
- 一旦将来把这个 store 加到继承链上（例如改挂 `FormModalStore`），必须切回模式 A（`makeObservable` + 显式装饰器）。

### 命名约定（两种模式通用）

- 私有字段：`_` 开头，外部不访问（`_roles`、`_menuTree`、`_selectedKeys`）。
- 公开读取：去掉 `_`（`roles`、`menuTree`、`selectedKeys`）。
- 修改动作：语义动词前缀（`loadRoles`、`setSelectedKeys`、`toggleCollapsed`）。

### 反例

```ts
// ❌ 继承链下用 makeAutoObservable —— 运行时报错
class BadStore extends FormModalStore<never> {
  constructor() {
    super();
    makeAutoObservable(this); // 禁止
  }
}

// ❌ 继承链下仍然用裸字段 —— 不会被 observe
class BadStore extends FormModalStore<never> {
  public roles: RoleItem[] = []; // 应改成 @observable private accessor _roles + @computed get roles
}

// ❌ 独立 store 里写装饰器 —— 多此一举，makeAutoObservable 已经处理了
class BadStore {
  @observable private accessor _roles: RoleItem[] = []; // 应去掉装饰器，改成 private _roles
  constructor() {
    makeAutoObservable(this);
  }
}

// ❌ 独立 store 把私有字段直接 public —— 失去只读边界
class BadStore {
  public roles: RoleItem[] = []; // 应改成 private _roles + 公开 getter
  constructor() {
    makeAutoObservable(this);
  }
}
```

## tsx 消费 store：解构 + getter，不要回调

**数据的所有权在 store；tsx 只订阅读取，不负责把数据塞回去。** store 的 fetch 方法不应接收「把结果回传给调用方」的回调；要暴露的值都走私有字段 + getter，tsx 用 `observer` 订阅即可。

### 正例

```tsx
const BindRoleModal: React.FC<Props> = observer(({ open, persId }) => {
  const { roles, checkedRoleIds, loadRoles, loadPersRoleIds } =
    BindRoleModalStore;

  useEffect(() => {
    if (!open) return;
    loadRoles();
  }, [open, loadRoles]);

  useEffect(() => {
    if (!open || !persId) return;
    loadPersRoleIds(persId);
  }, [open, persId, loadPersRoleIds]);

  return (
    <Checkbox.Group
      value={checkedRoleIds}
      options={roles.map((r) => ({ label: r.nm, value: r.id }))}
    />
  );
});
```

对应的 store：

```ts
@computed get roles() { return this._roles; }
@observable private accessor _roles: RoleItem[] = [];

@computed get checkedRoleIds() { return this._checkedRoleIds; }
@observable private accessor _checkedRoleIds: (number | string)[] = [];

public loadRoles = () => {
  getRoleList().then((res) => {
    if (res.code === 0) this._roles = res.data?.list || [];
    else this._message(res);
  });
};

public loadPersRoleIds = (persId: number | string) => {
  getPersRoleIds(persId).then((res) => {
    if (res.code === 0) this._checkedRoleIds = res.data?.list || [];
    else this._message(res);
  });
};
```

### 反例

```tsx
// ❌ fetch 回调把结果塞回 useState —— 数据源双份，永远会漂移
const [roles, setRoles] = useState<RoleItem[]>([]);
useEffect(() => {
  loadRoles((list) => setRoles(list));
}, [open, loadRoles]);
```

```ts
// ❌ store 方法接受结果回调透传
public loadRoles = (fn?: (list: RoleItem[]) => void) => {
  getRoleList().then((res) => fn?.(res.data?.list || []));
};
```

### 什么情况下仍然用回调？

回调只用在「调用方决定下一步 UI 动作」——这类回调不是传数据，而是通知时机。典型场景：

- `editFormData(id, data, () => { closeModal(); onOk?.(); })`——成功后由调用方决定关不关弹窗
- `delData(id, () => setSelectedKeys([]))`——删除后让调用方清理选中集

**判断标准：回调参数列表里有没有业务数据？** 有就是反模式（应该改成 store 私有字段 + getter），没有就是合法的"时机通知"回调。

### 两条推论

- tsx 里**不要** `useState` 保存从 store 读来的数据副本。
- 短命 UI 态（`open`、`title`、`editingId`）仍然用 `useState`——别把「store 数据不要 useState」误推广到「UI 态也不要 useState」。

## 静态内容页变体（`Panel.Default`）

仪表盘、说明页、图表聚合页等**非列表、非表单**的内容型页面，用 `Panel.Default` 作为外壳，而不是 `Panel.List`。这类页面通常没有搜索/分页/增删改，只需要 Panel 给它一个统一的边距、标题和内容区样式。

```tsx
import React from "react";

import { Panel } from "@hsu-react/ui";
import { observer } from "mobx-react-lite";
import styles from "./index.module.less";

const Xxx: React.FC = observer(() => {
  return (
    <Panel.Default
      className={styles.Xxx}
      contentClassName={styles.XxxContent}
    >
      {/* 内容在这里；图表、统计卡、子面板等 */}
    </Panel.Default>
  );
});

export default Xxx;
```

配套的 `index.module.less` 需要给**两个**选择器占位（外层 + 内容区）：

```less
.Xxx {
}

.XxxContent {
}
```

store 可以极简——**未继承任何基类的空 store 用 `makeAutoObservable`**：

```ts
import { makeAutoObservable } from "mobx";

class XxxStore {
  constructor() {
    makeAutoObservable(this);
  }
}

export default new XxxStore();
```

之后如果页面需要拉数据/加状态，按 "Store 状态暴露" 一节的 **模式 B · 独立 store（无继承）** 扩展即可（仍然 `makeAutoObservable` + 裸私有字段 + 裸 getter，不写装饰器）；如果后来发现要接列表/表单/分页，应当切换到 `Panel.List` + `ListPanelStore` 模板，而不是在 `Panel.Default` 里手写这些能力。

API 文件按页面路径镜像落一个同名 `.ts` 即可，哪怕内容暂时为空（由 api-creation skill 驱动），后续添函数时不用再动文件位置。

## 嵌套列表弹窗变体（`Panel.List.Modal`）

当一条"父记录"需要点进去管理一批"子记录"时（例如「异常策略」下管理「适用用户」），把子列表页做成一个 **Modal 里的列表** —— 外层父页面传入 `open` 和父级 ID，Modal 内部自行装一套完整的搜索 + 增删改。对应脚手架：`createListModalPanel.cjs`。

### 组件骨架

```tsx
import React, { useEffect, useState } from "react";

import { ChakraButtonProps, ColumnsType, FormItemProps, Panel, Operate } from "@hsu-react/ui";
import { PlusOutlined } from "@ant-design/icons";
import { observer } from "mobx-react-lite";

import XxxStore from "./XxxStore";
import XxxForm from "./XxxForm";
import styles from "./index.module.less";

interface XxxProps {
  open?: boolean;
  onCancel?: () => void;
  excepStratId?: number | string;  // 父级 ID 字段，按业务命名
}

const Xxx: React.FC<XxxProps> = observer((props) => {
  const { open: xxxOpen, onCancel: onXxxCancel, excepStratId } = props;
  const {
    setSearchData,
    initSearchData,
    dataSource,
    isLoading,
    total,
    changePage,
    page,
    getDataSource,
    delData,
    order,
    onOrderChange,
    resetStore,
  } = XxxStore;
  const [id, setId] = useState<number | string>();
  const [open, setOpen] = useState(false);
  const [title, setTitle] = useState("");

  useEffect(() => {
    if (xxxOpen && excepStratId) {
      initSearchData({ excepStratId });
    }
  }, [initSearchData, xxxOpen, excepStratId]);

  // searchItems / beforeButtonGroup / columns 同普通列表页

  return (
    <>
      <Panel.List.Modal
        open={xxxOpen}
        title="用户异常策略"
        onCancel={() => {
          resetStore();
          onXxxCancel?.();
        }}
        footer={false}
        className={styles.Xxx}
        searchProps={{ /* 同普通列表 */ }}
        tableProps={{ /* 同普通列表 */ }}
      />
      <XxxForm
        open={open}
        title={title}
        id={id}
        excepStratId={excepStratId}
        onCancel={() => { setId(""); setOpen(false); }}
        onOk={() => { getDataSource(); }}
      />
    </>
  );
});

export default Xxx;
```

要点（与普通 `Panel.List` 列表页的差异只在下面这几条，其余一致）：
- **props 命名必须消歧义**：外层父页面传进来的 `open`/`onCancel` 与内层"新增/编辑"表单的 `open`/`onCancel` 同名，`props` 解构时一律 rename：`open: xxxOpen`、`onCancel: onXxxCancel`。**不要**用 `modalOpen` 这类泛名——用模块名前缀，一眼对得上。
- **初始化条件加门**：`useEffect` 必须 `if (xxxOpen && <parentId>)` 双重判断后才 `initSearchData({ <parentId> })`，把父级 ID 透传进搜索参数；弹窗未打开或父级 ID 缺失时不要发请求。
- **关闭时 `resetStore()`**：`Panel.List.Modal` 的 `onCancel` 里**必须**先 `resetStore()` 再 `onXxxCancel?.()`，否则下次换个父级 ID 打开会带脏数据。普通列表页不需要这一步。
- **`footer={false}`** + `title="..."`：Modal 自带标题，底部按钮交给页面内 `Operate` 负责，不要双份。
- **子表单要接收并注入父级 ID**：`<XxxForm>` 多传一个 `excepStratId={excepStratId}`；表单组件侧按上面"表单弹窗"一节末尾"父级 ID 注入"要点处理。

### 配套 Store

`_modeType` 必须**显式声明**父级 ID 的比较模式（`EQ`），否则 `ListPanelStore` 会默认按模糊匹配拼参数：

```ts
class XxxStore extends ListPanelStore<XxxSearchData, XxxData> {
  protected accessor _modeType = {
    excepStratId: "EQ",
  };

  constructor() {
    super();
    makeObservable(this);
  }

  // getDataSource / delData 同普通列表页
}
```

其它与普通列表 store 一致，`resetStore` 由基类提供无需自己实现。

## 表格列宽规范

全项目的列宽是一个离散档位，不要随手填。新列建议先查下面的表对号入座，档位之间不要自由插值。对齐方式与 `fixedWidth` 也是配套约定（`fixedWidth: true` 要求本项目 `Table` 组件在水平滚动时严格固守该宽度）。

### 基本对齐约定

| 列类别 | `align` | `fixedWidth` | 备注 |
|---|---|---|---|
| 时间、枚举/状态、是否 X、数字/计数、ID/编号、手机号 | `"center"` | `true` | 行内内容定长或定值 |
| 名称/标题、备注/描述、URL/路径 | 不设（默认左） | 不设 | 内容变长自适应 |
| 操作 | `"center"` | `true` | 见下文专门一节 |

**排序字段翻译规避**：`dataIndex` 是翻译字段（`xxxDsr`、`xxxNm`）时务必加 `orderKey` 指向真正的 code 字段，避免按翻译文本排序。例如 `dataIndex: "typeDsr", orderKey: "type"`。

### 枚举/状态列：优先用后端 Dsr 字段，不要前端映射

列表列渲染字典型字段（状态、类型、是否、级别等）时，**直接用后端返回的 `xxxDsr`** 作为 `dataIndex`，不要在前端写 `render` 做 code→文案映射。项目中 `syslog/ApiLog`、`sysmgmt/sms/SmsLog`、`informationMaintenance/PersonnelManagement` 等绝大多数列表页都是这个写法。

```tsx
// ✅ 对
{ title: "状态", dataIndex: "statusDsr", width: 90, align: "center" }

// ❌ 错（硬编码映射，后端加第三态/改文案会失准）
{
  title: "状态",
  dataIndex: "status",
  render: (r) => (r.status === 0 ? "启用" : "停用"),
}
```

理由：
- 前端映射是脆弱契约——后端加 `status=2/3` 或改文案后 else 分支会错；
- 多页面/多系统间的文案应由后端统一出口，避免跨页面自行翻译导致分歧；
- 排序按文本还是按 code 由 `orderKey` 明确（见上条「排序字段翻译规避」），前端映射则两头都不稳。

**接口类型同步**：页面改用 `xxxDsr` 渲染后，对应的 `src/services/apis/<category>/<module>.ts` 里的 Data 接口也要补上 `xxxDsr?: string` 字段；否则 TS 虽然因 `MyData extends Record<string, unknown>` 放过了，但后续 review 看不到真实契约。

**例外**：后端暂时没给 Dsr 字段时，优先推动后端补字段；短期过渡可用 `Options("XXX_KEY")` + `OptionMap` 查表渲染，**绝不要**裸写 `record.x === 0 ? "A" : "B"`——那是最脆、最难维护的写法。

### Table 列 `render` 参数签名（极易踩坑）

本项目 Table 复用 antd 的列 render 签名。**参数含义按列是否设了 `dataIndex` 区分**：

- **有 `dataIndex`** 的列：`render(value, record, index)` —— **第一个参数是这一格的值（即 record[dataIndex]）**，第二个才是 record。
- **没有 `dataIndex`** 的列（如组合渲染、操作列、Tags 列）：`render(record, _, index)` —— 第一个参数就是 record。

实际项目里两种写法都存在，但**不能搞混**：在带 `dataIndex` 的列里写 `(record) => record.foo` 其实是把 value 当成 record 用，运行时 `value.foo === undefined`，整列会安静地全部渲染成"--"——TS 不会报错，肉眼也只看到列空着，是项目里最常见的"祖传 bug"。

```tsx
// ✅ 有 dataIndex —— 第一个参数是 value
{
  title: "预警百分比",
  dataIndex: "warnPercent",
  render: (value: number) =>
    value !== undefined && value !== null ? `${value}%` : "--",
}

// ✅ 有 dataIndex 但需要拿整行 —— 第二个参数才是 record（开关列就是这种）
{
  title: "是否启用",
  dataIndex: "status",
  render: (_, record) => <Switch defaultChecked={!!record.status} ... />,
}

// ✅ 没有 dataIndex —— 第一个参数就是 record
{
  title: "标签",
  render: (record) => <Tags tags={record?.tags?.map(t => t.nm)} />,
}

// ❌ 反例：有 dataIndex 却把第一个参数当成 record —— value 是数字，`value.warnPercent` 永远 undefined，整列显示 "--"
{
  title: "预警百分比",
  dataIndex: "warnPercent",
  render: (record) =>
    record.warnPercent ? `${record.warnPercent}%` : "--",
}
```

判断标准：**写之前问一句"我这列设没设 `dataIndex`？"**。设了就是 `(value) => ...` 或 `(_, record) => ...`；没设就是 `(record) => ...`。

### 时间列要 `dayjs` 格式化，列宽随精度

后端返回的时间字段格式并不统一：

- 框架级时间字段（`crtTm`/`updTm` 等）：很多模块由后端统一格式化成 `"YYYY-MM-DD HH:mm:ss"` 字符串，列里直接用 `dataIndex: "crtTm"` 即可，无需 `render`。
- 业务级时间字段（`rangeStartTm`/`rangeEndTm`、`xxxTime` 等）：后端经常原样返回 ISO `"2026-05-13T16:46:09"`，如果不处理，列里会带个突兀的 `T`。

业务时间列**必须**用 `dayjs(value).format(...)` 渲染，并根据**字段真实精度**选格式 + 列宽：

| 字段精度 | 表单端 | 渲染格式 | 列宽 |
|---|---|---|---|
| 仅日期（不含时分秒）| DATEPICKER 不传 `showTime` | `"YYYY-MM-DD"` | **120**（10 字符 + padding）|
| 日期 + 时分秒 | DATEPICKER `showTime: true` | `"YYYY-MM-DD HH:mm:ss"` | **160**（19 字符 + padding）|

判断规则：**看表单的 DATEPICKER 是否设 `showTime`**，前后端保持一致——表单里只让选日期的字段，列就不该多出 `00:00:00` 这段无用尾巴；表单选了时分秒的，列宽必须留到 160 才不会断行。

```tsx
import dayjs from "dayjs";

// 日期型字段（rangeStartTm 业务上只关心"哪一天"）
{
  title: "统计开始时间",
  dataIndex: "rangeStartTm",
  align: "center",
  width: 120,
  fixedWidth: true,
  render: (value: string) =>
    value ? dayjs(value).format("YYYY-MM-DD") : "--",
}

// 含时分秒（精确到秒的业务时间）
{
  title: "发生时间",
  dataIndex: "occurTm",
  align: "center",
  width: 160,
  fixedWidth: true,
  render: (value: string) =>
    value ? dayjs(value).format("YYYY-MM-DD HH:mm:ss") : "--",
}
```

后端字段是 LocalDateTime 但产品只需要日期粒度时，表单去掉 `showTime`、列用 `"YYYY-MM-DD"` + 120 宽即可，后端返回的尾部 `T00:00:00` 由 dayjs 自动吃掉。

### 非操作列宽度查表

| 列类别 | 标题样例 | 宽度 | 备注 |
|---|---|---|---|
| **时间列**（`crtTm`/`updTm`/`sendTm`/`xxxTm`）| 创建时间、修改时间、发送时间 | **160** | 固定 160，`"YYYY-MM-DD HH:mm:ss"` 的标准长度 |
| **枚举/状态列**（`xxxDsr` + `orderKey`）| 状态（2 字）| **80** | 短枚举 |
| | 类型、是否恢复、会话类型（3–4 字，值也短）| **90–100** | 按标题字数选 |
| | 调用状态、异常类型、频率配置（4 字且值较长）| **120–150** | 值更长时取 150 |
| **开关列**（`Switch` 渲染）| 是否启用 | **100** | `align: "center"`, `fixedWidth: true` |
| **ID/编号列**（`id`/`cd`/`cfgId`）| 用户ID、角色编码、配置ID | **180–200** | 主键/唯一码统一档位；**避免**把 ID 列设成 300+ |
| **手机号**（`mob`）| 手机号 | **120** | 固定 |
| **名称/标题列**（`nm`/`nickname`/`title`）| 名称、角色名称、部门名称、模板名称 | **150–200** | 默认 200；短名称或多列并排时可压到 150 |
| | 多值名称（角色列表、标签集合）、带链接的资产名 | **200–300** | 有 `render` 组合多个 Tag/Link 时取大档 |
| **备注/描述列**（`rmks`/`dsc`）| 备注、说明 | **200** | 默认 200；若同页已知普遍很长可给 240 |
| **长内容列**（`cont`/消息体/SQL）| 内容 | **300–500** | 内容可能溢出，配合 `TextEllipsis` 渲染 |
| **URL/路径/权限码列** | 调用URL、路由地址、权限标识 | **250–300** | 默认左对齐，不设 `fixedWidth` |
| **数字/计数列** | 策略数量、最大通知次数、排序值 | **80–140** | 2–3 位数 100，4 位以上 140；非可排序数值加 `sorter: false` |

### 档位选择的经验公式

若无合适档位可套，参考：

- 中心对齐列（时间/枚举/状态/ID/数字）：`width ≈ max(80, 标题字数 × 22 + 内容最长字数 × 16)`，然后就近向上对齐到 **80 / 100 / 120 / 140 / 160 / 180 / 200** 这组档位。
- 左对齐文本列（名称/备注/URL）：查表即可；实在拿不定默认 **200**。

### 列顺序惯例

列一般按"**标识 → 基本属性 → 枚举/状态 → 备注 → 时间 → 操作**"排列：

```
[rowSelection?] → 编码/ID → 名称 → 业务属性若干 → 类型/状态 → 备注 → 创建时间 → 修改时间 → 操作(fixed: "right")
```

首列不要加 `fixed: "left"`，除非该表左右滚动且主键列需常驻可见。操作列固定 `fixed: "right"`。

## 操作列宽度规范

操作列（`title: "操作"`）的宽度按"行内可见按钮的中文字符合计"查表，与全项目既有模块保持一致。基础属性固定：

```ts
{
  title: "操作",
  width: <按下表>,
  ellipsis: false,
  align: "center",
  fixed: "right",
  fixedWidth: true,
  render: (record) => (
    <Operate
      maxVisible={N}      // 见下方"maxVisible 规则"
      menu={[ /* 动作列表 */ ]}
    />
  ),
}
```

### maxVisible 规则

`<Operate>` 会把 `menu` 前 `maxVisible` 个按钮平铺在行内，其余折叠进尾部的 "..." 下拉按钮（称为"溢出"）。

- `menu.length ≤ 3`：可省略 `maxVisible`，全部按钮平铺。
- `menu.length ≥ 4`：必须显式写 `maxVisible`，通常取 2 或 3；不要让行内摆 4 个以上按钮（Role 的 `maxVisible={4}` 是历史例外，新模块不要模仿）。
- 有按钮受 `hidden`/`hasPermi` 动态控制时，仍按"设计上的最大可见数"算宽度，不要让宽度随数据抖动。

### 宽度查表

先算"可见字符数" `C` = 行内平铺按钮（前 `maxVisible` 个，未截断就是全部）的 `title` 中文字符合计；再判断是否"有溢出"（`menu.length > maxVisible`，即出现 "..." 下拉按钮，约占 20px）。

| 可见字符 C | 典型组合 | 无溢出 width | 有溢出 width |
|---|---|---|---|
| 2 | 1 × 2字（详情）| 80 | — |
| 4 | 2 × 2字（编辑/删除）| **140** | **160** |
| 6 | 3 × 2字（编辑/数据/删除）或 2字+4字（编辑/分配角色）| **190** | **190** |
| 8 | 3 个含 1 个 4 字（编辑/删除/用户策略）| **220** | **220** |
| 12 | 4 个 2/4 字混合（编辑/角色权限/数据权限/删除）| **300** | **300** |

**落档规则**：按 C 向上对齐到表中最接近的档位，不要在档位之间插值。例如 C=5 用 190，C=7 用 220。

**溢出 +20 只适用于 C=4 档**（140→160）；其它档位自带余量，溢出不再加宽。

### 快速示例

- 2 个 2 字按钮、无溢出 → `width: 140`（Dept / Param / SmsTemplate / TagManagement 等）
- `maxVisible={2}` + 3～5 个 2 字按钮 → `width: 160`（CodeGen / MqQueueManagement / ApiMetadataManagement）
- `maxVisible={2}` + 含 4 字按钮（编辑/分配角色，再折叠 2 个）→ `width: 190`（User）
- 3 个按钮全平铺、全 2 字 → `width: 190`（Dict / Menu / SmsLog）
- 3 个按钮含 1 个 4 字 → `width: 220`（DataEncryption / DataDesensitization / AppManagement / ApiExceptionStrategyManagement）
- 4 个按钮、`maxVisible={4}` → `width: 300`（Role，新模块尽量改用 `maxVisible={2~3}` 走 160/190/220）

## 开关/状态列快捷写法

表格中"是否启用/禁用"列不要额外弹窗，直接调用 `FormStore` 的 `editFormData`；
**成功回调用列表 store 的单行刷新，不要整表 `getDataSource()` 重拉**（整表重拉会置空列表 + loading、丢滚动位置，连续切换多行时反复闪烁）：

```tsx
{
  title: "是否启用",
  dataIndex: "status",
  render: (_, record) => (
    <Switch
      defaultChecked={!!record.status}
      options={Options("ENABLE_STATUS")}
      trueValue={1}
      falseValue={0}
      onChange={(checked) => {
        editFormData(
          record.id,
          { ...record, status: +checked },
          () => refreshRow(record.id)
        );
      }}
      key={`${record.id}-${record.status}`}
    />
  ),
  hasPermi: ["<module>:<sub>:upd"],
}
```

列表 store 侧公开 `refreshRow`，封装基类的 `_refreshRowData`（按主键经列表接口重拉该行并就地合并；内部自带乱序返回防护，未查到该行时自动回退整表刷新）：

```ts
public refreshRow = (id: number | string) => {
  this._refreshRowData(id, getXxxList);
};
```

反例：

```tsx
// ❌ 开关成功后整表重拉 —— 列表置空 + loading，滚动位置丢失，连续切换多行时反复闪烁
onChange={(checked) => {
  editFormData(record.id, { ...record, status: +checked }, () => getDataSource());
}}
```

## 导入顺序（项目风格）

大致遵循：
1. React / 第三方 React 生态（`@hsu-react/ui`、antd、antd icons、ahooks 等）
2. 项目 `@/services/...` / `@/stores/...`
3. `observer` from `mobx-react-lite`
4. 同目录相对导入（`./XxxStore`、`./XxxForm`）
6. `styles from "./index.module.less"` 放在靠后位置

组内空行区分即可，不必强制分段。现有文件里 import 顺序有轻微差异，以**同目录最近的兄弟文件**为准。

## Store 与 API 文件的一一对应

每个 store 只从**自己配对的**那个 API 文件导入。若多个 store 恰好都要用同一个后端地址，**在每个 store 对应的 API 文件里各自新增一个同名函数**，而不是跨模块去 import 别家 API 文件里的函数。

```ts
// ✅ XxxStore.ts 只从自己的 apis/<category>/xxx.ts 导入
import { getXxxList } from "@/services/apis/<category>/xxx";

// ❌ 不要从别的模块的 API 文件借东西
import { getYyyList } from "@/services/apis/<category>/yyy";
```

即便 `/some/endpoint` 这个 URL 在 `xxx.ts` 和 `yyy.ts` 里被重复写了两次，也接受这点重复——两份函数各自带着本模块的类型（`XxxData` vs `YyyData`），后续任一模块改字段、改 URL、改传参都不会牵连另一方。把 API 当作 store 的"私有出入口"看待，不要在 store 层做跨模块依赖。

## 编写流程（Claude 应遵循）

创建新 CRUD 页面时：

1. 用 Glob 扫当前项目 `src/pages/` 的子目录，确定归属大类（没有合适大类再新建）。
2. 读取同大类下 **1 个** 结构最完整的既有模块作为模板（优先挑有 Form 子目录的，如 `ApiBlacklistManagement/`），以其为蓝本复制并改名，这样能捕获本项目特有的组件名/类型名差异。
3. **确认对应 API**：按 api-creation skill 的路径镜像规则推导出 `src/services/apis/<category>/<modulename>.ts`（全小写连写）。若该 API 文件不存在或需要新增/修改函数，**通过 `Skill` 工具调起 `api-creation` skill**，由它驱动 API 层的落地，再回到页面侧接线。不要先写页面再事后补 API——这样能一次性把路径/命名对齐。
4. 按骨架产出 4 个文件：`index.tsx`、`index.module.less`（放一个空的 `.ModuleName {}` 占位即可）、`<ModuleName>Store.ts`、`<ModuleName>Form/` 下的三件套（如本次需要表单）。
5. `hasPermi` 权限码从后端/产品给的清单里取，**不要**自己拼——若用户没给，显式在代码顶部留一条 `// TODO: 确认权限码` 问回去（这是唯一允许的 TODO 场景）。
6. 别动路由注册文件——除非用户明确要求把新页面接到菜单/路由里。页面组件自身只负责渲染。
7. 创建后**不要**运行 build/lint，除非用户要求。

## 审计/Review 流程（Claude 应遵循）

当用户要求"检查这个页面是否合规"、"看看这几个页面是否符合规范"时：

1. 按本 skill 的骨架/命名/Store/反例逐项核对页面侧文件（`index.tsx`、`<Module>Store.ts`、`<Module>Form/` 等）。
2. **必须**同时通过 `Skill` 工具调起 `api-creation` skill，把页面对应的 `src/services/apis/<category>/<modulename>.ts` 文件纳入检查——路径是否镜像、文件名是否对应、函数命名、类型命名、`ListRes` 用法等。
3. 汇总两部分违规项一并产出报告；不要只给页面层面的结论而忽略 API 层的硬违规（比如 API 路径没镜像页面、文件名起了泛名）。

## 反例（常见错误，避免）

- ❌ 忘记加 `observer(...)` —— MobX 数据更新不会触发重渲染
- ❌ 把 `open`/`id` 这类短命 UI 态写进 MobX store —— 页面间切换会脏，用 `useState`
- ❌ 在 store 的 `.then` 里只设 `_isLoading = false` 不设 `.catch` —— 接口失败会卡 loading
- ❌ `export default XxxStore`（导出类）而不是 `export default new XxxStore()` —— 页面侧无法直接当单例用
- ❌ 模块文件夹或 Store 变量用 camelCase（`apiBlacklistManagement/`）—— 与项目约定（PascalCase）不一致
- ❌ 页面内写 `try/catch` + `notification.error` —— 失败提示由 store 的 `_message(res)` 或 axios 拦截器统一处理
- ❌ 用 `import XxxStore from "../../stores/XxxStore"` —— store 是页面私有时应放在页面文件夹内，而不是全局 stores
- ❌ 删除行后手动 splice `_dataSource` —— 直接 `getDataSource()` 重拉更简单也更不易出错
- ❌ 在 `_components/`/`_contComps/` 之外复用页面私有组件 —— 需要复用就升级到 `src/components/`，不要跨页直接 import 下划线目录的内容
- ❌ 从其它模块的 API 文件里 import 函数复用 —— 即使 URL 相同，也要在本 store 对应的 API 文件里各自新增一份，保持 store ↔ API 一一对应
