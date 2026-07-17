const path = require("path");
const webpack = require("webpack");
const TerserJSPlugin = require("terser-webpack-plugin");
const CssMinimizerPlugin = require("css-minimizer-webpack-plugin");
const { CleanWebpackPlugin } = require("clean-webpack-plugin");
const CopyWebpackPlugin = require("copy-webpack-plugin");
const MiniCssExtractPlugin = require("mini-css-extract-plugin");

const envPath = path.resolve(__dirname, `../.env/.env.prod`);
const envConfig = require("dotenv").config({ path: envPath }).parsed;

const definePlugin = {};
Object.keys(envConfig).map((key) => {
  definePlugin[`process.env.${key}`] = JSON.stringify(envConfig[key]);
});

// 路径处理函数
const normalizePath = (inputPath) => {
  return path.normalize(inputPath).replace(/\\/g, "/");
};

// 获取规范化的 cwd
const cwd = normalizePath(process.cwd());

const config = {
  mode: "production",
  devtool: false,
  output: {
    clean: true,
    path: path.resolve(__dirname, "../dist"),
    filename: (pathData) => {
      let name = pathData.chunk.name;
      if (name === "main") {
        return "static/js/[name].[hash:8].js";
      }
      return "static/js/[name].[hash:8].chunk.js";
    },
    chunkFilename: "static/js/[name].[hash:8].chunk.js",
    assetModuleFilename: "static/media/[ext]/[hash][ext][query]",
  },
  plugins: [
    new webpack.DefinePlugin(definePlugin),
    new MiniCssExtractPlugin({
      filename: (pathData) => {
        let name = pathData.chunk.name;
        if (name === "main") {
          return "static/css/[name].[hash:8].css";
        }
        return "static/css/[name].[hash:8].chunk.css";
      },
      // 异步 chunk 无 name，用 [id] 兜底（避免 name 渲染成字面量 undefined）
      chunkFilename: "static/css/[id].[hash:8].chunk.css",
    }),
    new CopyWebpackPlugin({
      patterns: [
        {
          from: normalizePath(path.resolve(__dirname, "../public")),
          to: normalizePath(path.resolve(__dirname, "../dist")),
          globOptions: {
            dot: true,
            gitignore: true,
            ignore: ["**/index.html"],
          },
          noErrorOnMissing: false,
          force: true,
          context: cwd, // 添加 context 配置
        },
      ],
    }),
    new CleanWebpackPlugin(),
  ],
  optimization: {
    splitChunks: {
      chunks: "all",
      cacheGroups: {
        vendors: {
          test: /[\\/]node_modules[\\/]/,
          name(module) {
            const packageName = module.context.match(
              /[\\/]node_modules[\\/](.*?)([\\/]|$)/
            )?.[1];
            // 匹配不到包名时返回 undefined，交给 webpack 用 chunk id 命名（避免出现字面量 "undefined"）
            return packageName ? packageName.replace("@", "") : undefined;
          },
          minSize: 20 * 1024,
          maxSize: 200 * 1024,
          minChunks: 1,
          priority: 100,
          reuseExistingChunk: true,
        },
      },
    },
    minimizer: [
      new TerserJSPlugin({
        extractComments: false,
        parallel: true,
        terserOptions: {
          compress: {
            drop_console: true,
            drop_debugger: true,
          },
        },
      }),
      new CssMinimizerPlugin({
        parallel: true,
      }),
    ],
  },
};

module.exports = config;
