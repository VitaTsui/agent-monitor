import "./install-object-has-own-polyfill";

import { installFreshnessGuard } from "./utils/freshness";
import { installKeyboardInset } from "./utils/keyboardInset";
import { installSheetSwipe } from "./utils/sheetSwipe";
import { hideSplashWhenReady } from "./utils/splash";

// 一进站就把钉钉绑定 token 从 URL 摘出暂存（登录跳转会丢查询参数），登录后由 Portal 消费绑定
installFreshnessGuard();
installKeyboardInset();
installSheetSwipe();

import "./index.scss";
// hsu-ui 全局样式（antd 观感覆盖），项目特有增量在本地 styles/antd-overload.scss
import "@hsu-react/ui/es/styles/antd-overload.scss";
// 设计令牌（shadcn 语义层）必须排在组件库样式之后：它有几支是指向 --vita-* 的，
// 而 --vita-* 由组件库的 tokens.scss 定义，先引会取不到值
import "./styles/tokens.scss";
import "./styles/antd-overload.scss";

import { BrowserRouter } from "react-router-dom";
// I18n 也收进了组件库（内置中英两套，并把 antd 的 locale 一并接上）
import HsuLayout from "@hsu-react/ui/es/layout";
import ClientTitleBar from "./layout/ClientTitleBar";
import ReactDOM from "react-dom/client";
import Routes from "./router/Routes";

import { SingleRouter } from "@hsu-react/single-router";

import { addCollection, type IconifyJSON } from "@iconify/react";
import phSubset from "./assets/iconify/ph.subset.json";

/* Phosphor 图标子集，**构建期打进产物**（4.4 KB，14 枚）。
 *
 * 状态图标（执行中 / 成功 / 失败 / 中断…）一律走 Phosphor，与 VitaAgent 逐字相同
 * （见 `pages/Portal/_components/StatusIcon`）；此前在 antd 图标里挑「语义最近的
 * 那一个」，尺寸颜色对齐过三轮，形状始终对不上 —— 两套图标集的字形本来就不同。
 *
 * **必须在这儿 `addCollection`，不能让它运行时去 Iconify 公共 API 拉。**
 * 这是个要在内网/离线环境跑的监控工具：运行时依赖外部 CDN，断网就是一片空白图标。
 * 注册过的名字 `@iconify/react` 一律走本地，不发任何请求。
 *
 * 子集由 `@iconify/json` 的 `ph.json` 裁出来（只留用到的那几枚），整集 4.3 MB
 * 不进产物、也不进依赖。要加图标：把对应条目补进 `assets/iconify/ph.subset.json`
 * （`viewBox` 必须与子集的 256×256 一致，否则不能直接并进去），再在用它的地方
 * 按名字引 —— 状态类的进 `StatusIcon` 的映射，非状态的直接用 `Icon icon="ph:xxx"`。 */
addCollection(phSubset as unknown as IconifyJSON);

ReactDOM.createRoot(document.getElementById("root")!).render(
  <BrowserRouter>
    <SingleRouter showPath={false}>
      <HsuLayout.I18n>
        {/* 外观控制器：它是唯一写 `html[data-theme]` 的地方，而全套设计令牌的暗色
            （组件库的 --vita-*、本项目 styles/tokens.scss 的 shadcn 语义层）都挂在
            那个属性上。原先它只包着 App（后管壳），于是前台 /portal、官网 /、登录页
            都不在它作用域内 —— 暗色令牌永远不会被激活，前台没有暗色模式可言。
            data-theme 是**文档级**的，写它的人就该在应用根上，一个就够。 */}
        <HsuLayout.Theme>
          <ClientTitleBar />
          <Routes />
        </HsuLayout.Theme>
      </HsuLayout.I18n>
    </SingleRouter>
  </BrowserRouter>,
);

// 首屏挂载后关掉原生启动图（消除白屏）
hideSplashWhenReady();
