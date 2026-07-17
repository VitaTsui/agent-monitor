import "./install-object-has-own-polyfill";

import "./index.scss";
// hsu-ui 全局样式（antd 观感覆盖），项目特有增量在本地 styles/antd-overload.scss
import "@hsu-react/ui/es/styles/antd-overload.scss";
import "./styles/antd-overload.scss";

import { BrowserRouter } from "react-router-dom";
import Internationalization from "./layout/I18n";
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
            <Routes />
          </Internationalization>
        </ChakraProvider>
      </CacheProvider>
    </SingleRouter>
  </BrowserRouter>,
);
