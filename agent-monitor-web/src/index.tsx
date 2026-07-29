import "./install-object-has-own-polyfill";

import { installFreshnessGuard } from "./utils/freshness";
import { installKeyboardInset } from "./utils/keyboardInset";
import { installSheetSwipe } from "./utils/sheetSwipe";
import { hideSplashWhenReady } from "./utils/splash";
import { stashDtbindToken } from "./utils/dtbind";

// 一进站就把钉钉绑定 token 从 URL 摘出暂存（登录跳转会丢查询参数），登录后由 Portal 消费绑定
stashDtbindToken();
installFreshnessGuard();
installKeyboardInset();
installSheetSwipe();

import "./index.scss";
// hsu-ui 全局样式（antd 观感覆盖），项目特有增量在本地 styles/antd-overload.scss
import "@hsu-react/ui/es/styles/antd-overload.scss";
import "./styles/antd-overload.scss";

import { BrowserRouter } from "react-router-dom";
import Internationalization from "./layout/I18n";
import ClientTitleBar from "./layout/ClientTitleBar";
import ReactDOM from "react-dom/client";
import Routes from "./router/Routes";

import { SingleRouter } from "@hsu-react/single-router";

import { ChakraProvider, createSystem, defaultConfig } from "@chakra-ui/react";
import createCache from "@emotion/cache";
import { CacheProvider } from "@emotion/react";

const cache = createCache({
  key: "css",
  prepend: true,
});

const system = createSystem(defaultConfig, {
  disableLayers: true,
  preflight: false,
});

ReactDOM.createRoot(document.getElementById("root")!).render(
  <BrowserRouter>
    <SingleRouter showPath={false}>
      <CacheProvider value={cache}>
        <ChakraProvider value={system}>
          <Internationalization>
            <ClientTitleBar />
            <Routes />
          </Internationalization>
        </ChakraProvider>
      </CacheProvider>
    </SingleRouter>
  </BrowserRouter>,
);

// 首屏挂载后关掉原生启动图（消除白屏）
hideSplashWhenReady();
