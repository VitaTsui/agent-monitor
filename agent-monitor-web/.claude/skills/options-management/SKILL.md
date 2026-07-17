---
name: options-management
description: Use this skill when adding, modifying, or consuming select/radio/checkbox options (dropdown 选项/枚举) in projects that follow the project-local `OptionsStore` convention — i.e. a project that has `src/stores/OptionsStore/` containing `index.tsx` (aggregator with an `OptionsType` union + an `options(key)` switch), `StaticOptions.ts`, and a `methods/` subdirectory of per-category classes. Typical trigger phrases: "新增一个下拉选项"、"加一个枚举"、"新增 select options"、"给 XX 字段接入选项"、"接入一个新的字典"、"Options 里加一项"、"OptionsStore 增加 XXX". Skip when the select options are one-shot local arrays unrelated to the shared store. The skill adapts to the current project's actual category files rather than assuming a fixed set.
---

# 选项管理规范 (src/stores/OptionsStore)

本 skill 规范选项（下拉、单选、复选的 `SelectOption[]`）在 `OptionsStore` 中的组织、注册与消费方式。目标是让新增的选项遵循现有的"私有槽 + 加载方法 + getter + 聚合器登记"四步流程。

## 先决条件 — 先读这些

新增选项前必须确认项目具备以下结构（若缺失，此 skill 不适用）：

- `src/stores/OptionsStore/index.tsx` —— 聚合类 `CommonOptions`，包含：
  - `OptionsType` 联合类型（所有可选 key）
  - 每个子方法类的私有实例字段与 `new` 实例化
  - 按类别委托的 `getXxx` 方法
  - `options(optionsType): SelectOption[]` 大 switch
  - 导出单例 `OptionsStore` 和可调用的 `Options`
- `src/stores/OptionsStore/StaticOptions.ts` —— 纯静态选项（硬编码数组）
- `src/stores/OptionsStore/methods/` —— 按业务类别拆分的 methods 类文件（`sysmgmt.ts`、`permit.ts`、`dataMonitoring.ts` 等；具体有哪些类别由当前项目决定，先用 Glob 查看）
- `@hsu-react/ui` 导出 `SelectOption` 类型（通常为 `{ label, value, ...extra }`）

## 三类选项，走不同路径

选项按来源分三类，新增时**先定位**自己属于哪一类：

### 1. 静态/写死的选项 → `StaticOptions.ts`

适用场景：选项固定、不随后端变化、跨页复用（如"是/否"、"HTTP 方法列表"、时间粒度等）。

```ts
export const YES_NO: SelectOption[] = [
  { label: "是", value: 1 },
  { label: "否", value: 0 },
];
```

命名：全大写 SCREAMING_SNAKE_CASE，常量名 = 业务含义。消费端直接 `import { YES_NO } from "@/stores/OptionsStore/StaticOptions";`，**不**走 `Options("...")` 也**不**进聚合器。

### 2. 后端字典（枚举）→ `methods/<category>.ts`，走 `getEnum`/`getEnumByPcd`

适用场景：后端字典表驱动的选项，通常 code 以 `XxxEn$Field` 形式组织（项目约定），或按 parent code 查。

```ts
public getSmsConfigStatus = () => {
  getEnum("SysSmsConfEn$Status").then((res) => {
    if (res.code === 0) {
      const { list } = res.data;
      this._smsConfigStatus = list?.map((item) => ({
        label: item.nm,
        value: item.cd,
      }));
    }
  });
};
```

key/value 约定：`label: item.nm`、`value: item.cd`（`cd` 是字典编码）。

### 3. 后端列表接口 → `methods/<category>.ts`，走专用 `getXxxList` API

适用场景：非字典、而是某资源列表做选择器（用户列表、角色列表、标签列表等）。

```ts
public getRole = () => {
  getRoleList().then((res) => {
    if (res.code === 0) {
      const { list } = res.data;
      this._role = list?.map((item) => ({
        label: item.nm,
        value: item.id,
      }));
    }
  });
};
```

key/value 约定：`label: item.nm`（或 `item.nickname`、`item.alias || item.nm`，按业务字段），`value: item.id`。若选项需要额外字段（keyType、sourceTypeId 等），**展开原 item**再覆盖 label/value：

```ts
this._smsConfigList = list?.map((item) => ({
  ...item,
  label: item.nm,
  value: item.id,
}));
```

## `methods/<category>.ts` 文件的骨架

每个 methods 类遵循固定的四段式：

```ts
import { getXxxList, getEnum } from "@/services/apis/enum";
import { SelectOption } from "@hsu-react/ui";
import { makeAutoObservable } from "mobx";

/**
 * XXX 相关的选项方法
 */
export default class XxxMethods {
  // ① 私有槽，每个选项一行，配业务注释
  private _someOption: SelectOption[] = [];

  constructor() {
    makeAutoObservable(this);
  }

  // ② 加载方法，public 箭头函数，命名 getXxx
  public getSomeOption = () => {
    getEnum("SomeEn$Field").then((res) => {
      if (res.code === 0) {
        const { list } = res.data;
        this._someOption = list?.map((item) => ({
          label: item.nm,
          value: item.cd,
        }));
      }
    });
  };

  // ③ getter 暴露只读访问
  get someOption() {
    return this._someOption;
  }
}
```

规则：
- **一个 `methods/<category>.ts` 文件对应一个业务类别**。类别由项目现有文件决定，先 Glob 扫 `methods/`，匹配不上再新建。
- 字段全部 `private _xxx`，camelCase；业务注释写在私有槽上方一行。
- `makeAutoObservable(this)` 放构造器，不手工标注 `@observable`——与项目其它 methods 类一致。
- 加载方法用箭头属性而非类方法（保 this 绑定），**不要**写 `async/await`，统一 `.then(res => { if (res.code === 0) {...} })`。失败分支不处理——加载失败保持空数组。
- `list?.map(...)` 用可选链，避免后端返回 `null` 时报错。
- 支持参数的加载方法（如按 projId 过滤）在参数层接收，但结果仍写回同一私有槽；调用方负责在需要时重新 `getXxx(newParam)` 覆盖。
- 不要在 methods 类里放非选项数据；Options 是 Select 专用，其它业务数据属于各自页面的 store。

## 聚合器 `index.tsx` 的登记（四处联动修改）

每加一个动态选项，必须在 `index.tsx` 的四个位置同步修改，否则 `Options("KEY")` 拿不到数据：

1. **顶部 JSDoc**：在 `CommonOptions` 类上方的大块注释里加 `@param KEY 中文含义`。
2. **`OptionsType` 联合**：在联合类型中追加 `| "KEY"`。KEY 命名：SCREAMING_SNAKE_CASE，与业务含义对齐；通常与方法名呼应（`getEnableStatus` ↔ `ENABLE_STATUS`）。
3. **委托方法**：在对应类别的"// XXX 相关方法 - 委托给 XxxMethods"段内，添加 `public getKey = () => this.xxxMethods.getKey();`。若方法带参数，透传：`public getKey = (p?: string) => this.xxxMethods.getKey(p);`。
4. **`options()` switch**：在大 switch 中追加 `case "KEY": return this.xxxMethods.xxxField;`——**注意 case 用 KEY，返回的是 methods 实例的 getter**。

静态选项（`StaticOptions.ts`）不需要登记进聚合器。

## 消费端用法（使用方法）

### 动态选项（走聚合器）

在组件里分两步：加载 + 取值。

```tsx
import OptionsStore, { Options } from "@/stores/OptionsStore";

const Demo: React.FC = observer(() => {
  const { getEnableStatus, getRole } = OptionsStore;

  useEffect(() => {
    getEnableStatus();
    getRole();
  }, [getEnableStatus, getRole]);

  const formItems: FormItemProps[] = [
    {
      type: "SELECT",
      name: "roleId",
      label: "角色",
      componentProps: { options: Options("ROLE") },
    },
    {
      type: "RADIO",
      name: "status",
      label: "是否启用",
      componentProps: { options: Options("ENABLE_STATUS") },
    },
  ];

  // ...
});
```

要点：
- `Options` 是可调用函数，不是对象；`Options("KEY")` 返回 `SelectOption[]`。
- **加载方法必须在 `useEffect` 里显式调一次**，否则数据为空数组。一个页面用到几个 key，就在 useEffect 里列几个。
- 把 `getXxx` 作为 useEffect 依赖（`[getXxx]`）——它们是稳定引用（挂在单例上），不会导致重复触发。
- 不要把 `Options("KEY")` 放到渲染函数外层常量——它需要 observer 包裹才能响应数据回来后的更新。

### 静态选项

直接 import：

```tsx
import { YES_NO, HOME_TIME_TYPE } from "@/stores/OptionsStore/StaticOptions";

// 组件里直接用
componentProps: { options: YES_NO }
```

## 编写流程（Claude 应遵循）

新增一个选项时：

1. **定类**：静态？字典？列表？
   - 静态 → 跳到 StaticOptions.ts，加完即完。
   - 字典/列表 → 进 methods 流程。
2. **定类别**：Glob 扫 `src/stores/OptionsStore/methods/` 看当前项目有哪些 methods 文件，挑语义最接近的归入；都不匹配再新建 `<newCategory>.ts` 并在 `index.tsx` 里追加对应的 import、实例字段、new 调用、委托段。
3. **定服务层**：确认对应 API 已在 `src/services/apis/enum.ts` 或相应服务模块里导出；没有的话先按 api-creation skill 把 API 加上。
4. **改 methods 文件**：四段式—— 私有槽 + 加载方法 + getter。
5. **改 index.tsx 四处**：JSDoc + OptionsType + 委托方法 + options switch case。
6. **消费端接入**：页面 `useEffect` 里调 `getXxx`，`FormItem.componentProps.options` 填 `Options("KEY")`。

## 反例（常见错误，避免）

- ❌ 直接把动态选项硬塞进 `StaticOptions.ts` —— 数据后端驱动的必须走 methods。
- ❌ 在聚合器 switch 里加了 case 但忘了加 `OptionsType` 联合 —— TypeScript 会报错；加了联合但忘了 switch case，`Options("KEY")` 默认返回 `[]` 却不会报错，更隐蔽。
- ❌ methods 类里写 `makeObservable(this) + @observable` —— 项目统一用 `makeAutoObservable`，不要混搭。
- ❌ 加载方法里写 `try/catch` 或 `notification.error` —— 选项加载失败应静默留空，不应打扰用户。
- ❌ 在 JSX 渲染里内联 `useEffect(() => getXxx())` 之外再调 `getXxx()` —— 会反复触发请求。加载只在挂载或关键依赖变化时一次。
- ❌ label/value 字段自创（`text`、`key`）—— `SelectOption` 固定 `label`/`value`，其它字段通过 `...item` 展开保留。
- ❌ 把选项 state 写在页面的业务 store 里 —— 跨页复用时会重复请求，应走共享的 OptionsStore。
- ❌ `getXxx` 命名与 KEY 不对应（如 `loadRoles` ↔ `ROLE`）—— 保持一致：`getRole` ↔ `ROLE`、`getEnableStatus` ↔ `ENABLE_STATUS`，便于搜索与维护。
- ❌ 同一 key 多次写入不同槽 —— 一个 KEY 对应唯一的私有槽，按参数过滤时复写同一槽即可。
