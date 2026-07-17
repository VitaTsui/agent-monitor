const { merge } = require("webpack-merge");
const commonConfig = require("./webpack.config.common.cjs");
const productionConfig = require("./webpack.config.prod.cjs");
const developmentConfig = require("./webpack.config.dev.cjs");
const portfinder = require("portfinder");
const FriendlyErrorsWebpackPlugin = require("@nuxt/friendly-errors-webpack-plugin");

module.exports = (env) => {
  switch (true) {
    case env.development:
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
      const _prodConfig = merge(commonConfig, productionConfig);
      return _prodConfig;
    default:
      return new Error("No matching configuration was found");
  }
};
