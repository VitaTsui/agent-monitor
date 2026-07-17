---
name: api-creation
description: Use this skill when creating, adding, or modifying API modules under `src/services/apis/` in any project that has `src/services/Axios.ts` exporting `get`/`post`/`del`/`put` and `src/services/ResType.ts` exporting `ListRes`. Trigger phrases include "新增一个接口"、"创建 API"、"add a CRUD api"、"在 services/apis 下新增模块"、"新增一个模块的增删改查", or any request to wire up backend endpoints for a list/detail/create/update/delete page. Skip for API usage inside `pages/`/`components/` — this skill is only for authoring files inside `src/services/apis/`. The skill adapts to the current project's own directory structure rather than assuming a fixed set of categories.
---

# API 创建规范 (src/services/apis)

本 skill 规范 `src/services/apis/` 下 API 文件的创建方式，确保与项目现有风格一致。

## 先决条件 — 先读这些

每次创建 API 前，必须先确认项目具备以下基础设施（若缺失，则此 skill 不适用）：

- `src/services/Axios.ts`：导出 `get`、`post`、`del`、`put`、`streamRequest`，以及 `ResType<T>` 接口。`get`/`post` 返回 `Promise<ResType<T>>`，其中 `T` 是业务 data 的类型。
- `src/services/ResType.ts`：导出 `ListRes<T>`（`{ list, page: { pageNum, pageSize, total } }`）和 `FileRes`（`{ filename, data }`）。
- `src/services/Query.ts`：查询条件构造器，通常以 `new Query().value` 的形式作为 `params.query` 传入列表接口。

导入路径固定使用别名 `@/services/Axios` 和 `@/services/ResType`，**不要**使用相对路径（`../Axios`）——后者仅在 `apis/` 根下的旧文件存在，新文件一律走别名。

## 模块文件的骨架

每个 API 模块（对应一个业务功能，如"API 黑名单管理"）是一个独立的 `.ts` 文件，遵循统一骨架：

```ts
/**************************************
 * Module Name : <中文模块名>
 **************************************/

import { get, post } from "@/services/Axios";

import { ListRes } from "@/services/ResType";

// 表单/页面搜索态类型 —— 供页面组件使用，字段宽松
export interface XxxSearchData extends Record<string, unknown> {
  // 具体字段...
}

// 请求参数类型 —— 实际发给后端的 query 参数形状
interface XxxSearch extends Record<string, unknown> {
  query: string;
  // 其它可选查询字段...
}

// 实体完整形状 —— 定义为 internal interface，再用 Partial 暴露
interface IXxxData {
  id: string | number;
  // ... 后端返回的所有字段，类型尽量精确
}
export type XxxData = Partial<IXxxData>;

// 列表
export const getXxxList = async (params: XxxSearch) => {
  return await get<ListRes<XxxData>>("/<模块前缀>/page", { params });
};

// 详情
export const getXxx = async (id: number | string) => {
  return await get<XxxData>("/<模块前缀>/info/" + id);
};

// 新增
export const createXxx = async (data: XxxData) => {
  return await post("/<模块前缀>/add", data);
};

// 修改
export const editXxx = async (data: XxxData) => {
  return await post("/<模块前缀>/upd", data);
};

// 删除
export const deleteXxx = async (id: number | string) => {
  return await get("/<模块前缀>/del", { params: { ids: id } });
};
```

## 命名规范（必须严格遵守）

### 目录和文件名 — 必须与页面路径一一镜像

**页面就是大类。** `src/services/apis/` 下的目录/文件结构必须与 `src/pages/` 下对应页面的路径一一对应，文件名就是页面模块目录名的"全小写连写"形式。

| 页面路径 | 对应 API 路径 |
|---|---|
| `src/pages/HeatmapManagement/` | `src/services/apis/heatmapmanagement.ts` |
| `src/pages/EnergyReport/` | `src/services/apis/energyreport.ts` |
| `src/pages/informationMaintenance/HonorManagement/` | `src/services/apis/informationmaintenance/honormanagement.ts` |
| `src/pages/gbhx/RoleManagement/` | `src/services/apis/gbhx/rolemanagement.ts` |
| `src/pages/gbhx/MenuManagement/` | `src/services/apis/gbhx/menumanagement.ts` |

规则：
- **大类目录 = 页面大类目录**：页面有多少层 category，API 目录就有多少层（同名，全小写）。
- **文件名 = 页面模块目录名的全小写连写**：去掉空格/连字符/驼峰，例如页面 `HeatmapManagement/` → API 文件 `heatmapmanagement.ts`，页面 `PersRoleBinding/` → `persrolebinding.ts`。
- **不要起泛名**：`menu.ts`、`role.ts`、`auth.ts` 这种名字与页面模块名不对应、容易和项目其它概念撞义，禁止使用。
- **跨页面共享的 API**（比如登录、改密、全局权限探测等没有对应业务页面的接口）直接放在 `src/services/apis/` 根下，用 `login.ts`、`pwdchange.ts`、`permit.ts` 这种语义清晰的短名。
- **复杂模块（页面目录下有子页面）**：API 目录结构也跟着嵌套。如页面 `ApiMetadataManagement/ApiMetadataDetail/` 对应 API `apimetadatamanagement/apimetadatadetail.ts`（或继续平铺 `apimetadatamanagementdetail.ts`，取决于子页面是否独立调用自己的一组 CRUD 接口）。

### 其它命名

- **类型名（PascalCase）**：
  - 页面表单搜索态：`XxxSearchData`（导出，`extends Record<string, unknown>`）
  - 请求 search 参数：`XxxSearch`（内部 interface，`extends Record<string, unknown>`，`query: string` 通常必填）
  - 实体全字段：`IXxxData`（internal，前缀 `I`）
  - 对外暴露的实体：`XxxData = Partial<IXxxData>`（导出 type 别名）
- **函数名（camelCase，CRUD 固定前缀）**：
  - 列表 → `getXxxList`
  - 详情 → `getXxx`
  - 新增 → `createXxx`
  - 修改 → `editXxx`
  - 删除 → `deleteXxx`
  - 状态变更 → `updateXxxStatus`
  - 批量操作 → `batchSaveXxxRole` 等，语义前缀 + 动作

## 方法与 URL 约定

- 所有列表接口：`GET /<模块前缀>/page`，`params` 中通常携带 `query`（由 `Query` 类编码得到的字符串）
- 所有详情接口：`GET /<模块前缀>/info/{id}`（路径参数）或 `GET /<模块前缀>/dtl`（query 形式，按后端约定）
- 新增：`POST /<模块前缀>/add`
- 修改：`POST /<模块前缀>/upd`（少数模块新增/修改共用 `/save`，遵循后端实际约定）
- 删除：`GET /<模块前缀>/del?ids={id}`（**注意：删除走 GET，参数名是 `ids`**，与直觉相反，但这是项目约定）
- 简单枚举/下拉列表：`GET /<模块前缀>/list`，返回 `ListRes<{ id, nm }>` 或 `ListRes<{ cd, nm }>`

## 两种常见 API 形态的选择

1. **完整 CRUD 模块**：用上面的完整骨架。
2. **只读枚举/下拉**（若项目存在 `apis/enum.ts`，应归并进去，而**不是**新建文件）：
   ```ts
   export const getXxxList = async () => {
     return await get<ListRes<{ id: string | number; nm: string }>>("/<path>/list");
   };
   ```
   新增此类枚举前，先查看 `apis/enum.ts` 是否已有类似函数或合适位置。

## 返回类型

- 列表：统一用 `get<ListRes<XxxData>>(...)`——**分页和非分页 list 接口都用同一个 `ListRes<T>` 泛型**。后端对非分页接口只返 `{ list }` 不返 `page` 是常见情况；前端侧仍然套 `ListRes<T>`，消费者按需取 `list` 即可，不要另造 `{ list: T[] }` 这种手写形状（会让类型在项目里碎片化）。只在消费端真要用 `page` 字段时再做 `page?.total ?? 0` 之类的可选访问。
- 详情/单对象：`get<XxxData>(...)` 或 `post<XxxData>(...)`
- 无业务 data：省略泛型，`post(...)` 即可——返回 `ResType<undefined>`
- 文件下载：调用方在 `config` 中传 `responseType: "blob"`/`"arraybuffer"`，此时响应会被 `Axios.ts` 自动包装成 `FileRes`（`{ filename, data }`），类型写 `get<FileRes>(...)`（需要 `import { FileRes } from "@/services/ResType"`）。
- 流式 SSE：使用 `streamRequest`，不要套 `get`/`post`。

## 导入与样式细节

- `import { get, post } from "@/services/Axios";` 和 `import { ListRes } from "@/services/ResType";` 之间保留**一个空行**——这是项目里 CRUD 文件的固定写法。
- 模块名注释块（`/************** Module Name : ... **************/`）放在所有 import 之前。
- 不要在文件里写 `// TODO`、`// FIXME` 之类的占位注释。
- 不要手写请求拦截、token 注入、错误提示——这些都在 `Axios.ts` 里统一处理。

## 编写流程（Claude 应遵循）

当用户要求新增一个 API 模块时：

1. **确定对应页面路径**：该 API 是给哪个页面用的？先拿到 `src/pages/<...>/<ModuleName>/` 的完整路径。若还没有对应页面（纯跨页共享 API、登录/改密等全局入口），才允许脱离页面镜像规则，平铺在 `src/services/apis/` 根下。
2. **按页面路径镜像推导 API 路径**：页面 `src/pages/A/B/<ModuleName>/` → API `src/services/apis/a/b/<modulename>.ts`（大类目录 = 页面大类全小写；文件名 = 页面模块名全小写连写）。
3. 用 Glob 检查**当前项目** `src/services/apis/` 的现有结构，确认镜像目标是否已存在——存在则直接新增文件；不存在则同时新建镜像目录。如果既有文件名偏离了镜像规则（例如 `menu.ts` / `role.ts` 这种泛名），在审计/Review 时应提示重命名。
4. 读取同目录下 **1 个** 最相近的既有 CRUD 文件，以其为模板——这可以捕获本目录可能有的微小风格差异。
5. 从用户提供的字段/接口定义中提炼 `IXxxData`、`XxxSearch`、`XxxSearchData`。未知字段留到后续再补，不要瞎编。
6. 产出文件，文件名遵循"全小写连写 + 镜像页面模块名"规则。
7. 不要主动在 `index.ts` 做再导出——项目不使用 barrel 文件。调用处直接 `import { getXxxList } from "@/services/apis/<dir>/<file>"`。
8. 创建后**不要**运行 lint/build 除非用户要求——这些接口文件通常会被页面引用后才编译到。

## 反例（常见错误，避免）

- ❌ 用相对路径 `import { get } from "../../Axios"` —— 应改为 `@/services/Axios`
- ❌ 文件名用驼峰 `apiBlacklistManagement.ts` —— 应改为全小写 `apiblacklistmanagement.ts`
- ❌ 文件名起泛名 `menu.ts`、`role.ts`、`auth.ts` —— 必须镜像页面模块名，如 `menumanagement.ts`、`rolemanagement.ts`；只有真正跨页共享的 API 才允许用短名，且放在 `apis/` 根下
- ❌ API 目录结构与 `src/pages/` 下对应页面路径不一致 —— 页面大类目录必须在 API 层镜像一份
- ❌ 删除接口用 `del("/xxx/del/" + id)` —— 项目统一用 `get("/xxx/del", { params: { ids: id } })`
- ❌ 直接导出 `interface XxxData` —— 应导出 `type XxxData = Partial<IXxxData>`，保留内部 `IXxxData` 作为完整形状
- ❌ 非分页列表接口手写 `{ list: T[] }` 类型 —— 仍然用 `ListRes<T>`，把碎片化收敛掉
- ❌ 在 API 文件里写 `try/catch` 或 `notification.error` —— 全局拦截器已处理
- ❌ 把简单的下拉列表接口单独开文件 —— 应追加到 `apis/enum.ts`
- ❌ 在 `apis/` 下放一次性种子/引导脚本（`_initXxx.ts`） —— 脚本属于 `src/utils/` 或项目级 `scripts/`，不进 API 层
