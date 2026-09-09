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
