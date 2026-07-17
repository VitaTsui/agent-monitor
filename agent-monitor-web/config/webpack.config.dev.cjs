const path = require("path");
const webpack = require("webpack");
const ReactRefreshWebpackPlugin = require("@pmmmwh/react-refresh-webpack-plugin");
const ForkTsCheckerWebpackPlugin = require("fork-ts-checker-webpack-plugin");
const MiniCssExtractPlugin = require("mini-css-extract-plugin");

const envPath = path.resolve(__dirname, `../.env/.env.dev`);
const envConfig = require("dotenv").config({ path: envPath }).parsed;

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
