const path = require("path");
const webpack = require("webpack");
const ReactRefreshWebpackPlugin = require("@pmmmwh/react-refresh-webpack-plugin");
const ForkTsCheckerWebpackPlugin = require("fork-ts-checker-webpack-plugin");
const MiniCssExtractPlugin = require("mini-css-extract-plugin");

// .env.dev 不入库（带本机代理目标与登录密钥），干净 clone 里没有这个文件。
// dotenv 缺文件时 parsed 给的是 {} 不是 undefined，不拦一道的话 dev-server 照常
// 起来、只是没有 /api 代理也没有登录密钥，然后在浏览器里以各种奇怪方式失败。
const envPath = path.resolve(__dirname, `../.env/.env.dev`);
const dotenvResult = require("dotenv").config({ path: envPath });
const envConfig = dotenvResult.parsed || {};
if (dotenvResult.error) {
  throw new Error(
    `缺少 ${envPath}。先 cp .env/.env.dev.example .env/.env.dev，` +
      `再按 docs/local-dev-setup.md 第一节把 CRYPTO_KEY / RSA_PUB_KEY 换成本机 hub 打印的值。`
  );
}

const definePlugin = {};
const api_proxy = {};
Object.keys(envConfig).map((key) => {
  definePlugin[`process.env.${key}`] = JSON.stringify(envConfig[key]);

  if (key === "API_PROXY") {
    const _api_proxy = JSON.parse(envConfig[key]);
    Object.keys(_api_proxy).map((key) => {
      api_proxy[key] = {
        target: _api_proxy[key]?.target,
        pathRewrite: _api_proxy[key]?.pathRewrite,
        changeOrigin: true,
        // 放行 WebSocket 升级请求；对纯 HTTP 的代理没有副作用
        ws: true,
      };

      if (_api_proxy[key]?.ReqHeader) {
        api_proxy[key].onProxyReq = (proxyReq) => {
          Object.keys(_api_proxy[key].ReqHeader).map((_key) => {
            proxyReq.setHeader(_key, _api_proxy[key].ReqHeader[_key]);
          });
        };
      }
    });
  }
});

const config = {
  mode: "development",
  devtool: "inline-source-map",
  plugins: [
    new webpack.DefinePlugin(definePlugin),
    new MiniCssExtractPlugin(),
    new ForkTsCheckerWebpackPlugin({
      typescript: {
        memoryLimit: 4096,
        diagnosticOptions: { semantic: true, syntactic: true },
      },
      async: true,
    }),
    new ReactRefreshWebpackPlugin({ overlay: false }),
  ],
  stats: "errors-only",
  devServer: {
    client: {
      logging: "error",
      overlay: false,
    },
    historyApiFallback: true,
    compress: false,
    hot: true,
    port: [envConfig.SERVER_PROT],
    proxy: api_proxy,
  },
};

module.exports = config;
