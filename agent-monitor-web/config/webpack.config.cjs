const { merge } = require("webpack-merge");
const commonConfig = require("./webpack.config.common.cjs");
// dev / prod 两份配置在模块作用域各自读自己的 .env.*（dev 读 .env.dev、prod 读
// .env.prod），所以只能用哪份 require 哪份：以前两份都在顶部 require，跑生产构建
// 也要求本机存在 .env.dev，反之亦然。两个文件都不入库，谁少一个都会把另一边的
// 构建一起带崩。
const portfinder = require("portfinder");
const FriendlyErrorsWebpackPlugin = require("@nuxt/friendly-errors-webpack-plugin");

module.exports = (env) => {
  switch (true) {
    case env.development:
      const developmentConfig = require("./webpack.config.dev.cjs");
      const _devConfig = merge(commonConfig, developmentConfig);

      return new Promise((resolve, reject) => {
        portfinder.getPort(
          {
            port: _devConfig.devServer.port,
            stopPort: 65535,
          },
          (err, port) => {
            if (err) {
              reject(err);
              return;
            }
            _devConfig.devServer.port = port;
            _devConfig.plugins.push(
              new FriendlyErrorsWebpackPlugin({
                compilationSuccessInfo: {
                  messages: [`Local:   http://localhost:${port}/`, ""],
                },
              })
            );
            resolve(_devConfig);
          }
        );
      });
    case env.production:
      const productionConfig = require("./webpack.config.prod.cjs");
      const _prodConfig = merge(commonConfig, productionConfig);
      return _prodConfig;
    default:
      return new Error("No matching configuration was found");
  }
};
