const path = require("path");
const webpack = require("webpack");
const TerserJSPlugin = require("terser-webpack-plugin");
const CssMinimizerPlugin = require("css-minimizer-webpack-plugin");
const { CleanWebpackPlugin } = require("clean-webpack-plugin");
const CopyWebpackPlugin = require("copy-webpack-plugin");
const MiniCssExtractPlugin = require("mini-css-extract-plugin");

// .env.prod 不入库（含与线上 hub 配对的 CRYPTO_KEY / RSA_PUB_KEY），
// 由 scripts/write-env-prod.sh 从 .env.prod.example 生成。
//
// 这里必须自己拦一道：文件缺失时 dotenv 只把 error 挂在返回值上，parsed 给的是 {} ——
// 构建照样成功，只是所有 process.env.* 被替换成 undefined，跑起来登录直接崩，
// 而且崩在浏览器里、离构建现场十万八千里。宁可现在就编不过。
const envPath = path.resolve(__dirname, `../.env/.env.prod`);
const dotenvResult = require("dotenv").config({ path: envPath });
const envConfig = dotenvResult.parsed || {};
const missing = ["NODE_ENV", "CRYPTO_KEY", "RSA_PUB_KEY"].filter(
  (k) => !envConfig[k]
);
if (dotenvResult.error || missing.length) {
  throw new Error(
    `${envPath} ${dotenvResult.error ? "不存在或读不了" : `缺少 ${missing.join("、")}`}。先生成它：\n` +
      `  CRYPTO_KEY=… RSA_PUB_KEY=… bash scripts/write-env-prod.sh\n` +
      `（值必须与线上 hub 的 AM_CRYPTO_KEY / RSA 私钥配对，见 .env/.env.prod.example）`
  );
}

const definePlugin = {};
Object.keys(envConfig).map((key) => {
  definePlugin[`process.env.${key}`] = JSON.stringify(envConfig[key]);
});
// 构建号：页面运行时对比服务端 build-id.txt，不一致自动强刷 ——
// 根治 WKWebView（iOS 壳/客户端）拿旧缓存页的顽疾
const BUILD_ID = Date.now().toString(36);
definePlugin["process.env.BUILD_ID"] = JSON.stringify(BUILD_ID);

class EmitBuildIdPlugin {
  apply(compiler) {
    compiler.hooks.thisCompilation.tap("EmitBuildId", (compilation) => {
      compilation.hooks.processAssets.tap(
        { name: "EmitBuildId", stage: compiler.webpack.Compilation.PROCESS_ASSETS_STAGE_ADDITIONAL },
        () => {
          compilation.emitAsset(
            "build-id.txt",
            new compiler.webpack.sources.RawSource(BUILD_ID),
          );
        },
      );
    });
  }
}

// 路径处理函数
const normalizePath = (inputPath) => {
  return path.normalize(inputPath).replace(/\\/g, "/");
};

// 从模块所在目录推出它属于哪个 npm 包，用作 vendor chunk 名。
//
// 必须从「最后一个」node_modules 往后取：pnpm 的物理布局是
//   node_modules/.pnpm/<pkg>@<ver>/node_modules/<pkg>/…
// 从头匹配只会取到 ".pnpm" 这一段，于是全部 vendor 落进同一个名为 ".pnpm" 的组，
// 再被 maxSize 切成一堆 ".pnpm-xxxxxxxx.chunk.js" —— 以点开头的文件会被任何按
// 「点开头 = 隐藏文件」处理的工具静默丢掉（GitHub Actions 的 upload-artifact
// 默认就丢，且丢完仍报成功，桌面客户端因此拿到过残缺的前端产物）。
// 取最后一个 node_modules 之后的段，pnpm 嵌套布局与 npm/yarn 扁平布局都成立。
const NODE_MODULES = "/node_modules/";
const packageNameOf = (context) => {
  if (!context) return undefined;
  const p = context.replace(/\\/g, "/");
  const i = p.lastIndexOf(NODE_MODULES);
  if (i < 0) return undefined;
  const segments = p.slice(i + NODE_MODULES.length).split("/");
  // scoped 包占两段（@scope/name），普通包一段
  const raw = segments[0]?.startsWith("@")
    ? segments.slice(0, 2).join("/")
    : segments[0];
  // 取不到、或仍以点开头（.pnpm / .bin 这类内部目录）时返回 undefined，
  // 交给 webpack 用 chunk id 命名 —— 数字 id 同样不会以点开头
  if (!raw || raw.startsWith(".")) return undefined;
  // @scope/name -> scope-name；其余文件名非法字符统一换成 -
  const name = raw.replace(/^@/, "").replace(/[^A-Za-z0-9_.-]+/g, "-");
  return name && !name.startsWith(".") ? name : undefined;
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
    new EmitBuildIdPlugin(),
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
            return packageNameOf(module.context);
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
