import { createHash } from "node:crypto";
import path from "node:path";
import { fileURLToPath } from "node:url";

import react from "@vitejs/plugin-react";
import {
  defineConfig,
  type Plugin,
  type ProxyOptions,
  type UserConfig,
} from "vite";
import checker from "vite-plugin-checker";
import dotenv from "dotenv";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const r = (p: string) => path.resolve(__dirname, p);

/**
 * 读 .env/ 下的自定义环境文件。
 *
 * 这个项目的环境文件不在根目录、也不叫 .env.development —— 是 .env/.env.common 与
 * .env/.env.dev|prod，而且值里放了 JSON（API_PROXY、PASS_CLS）、变量名也没有 VITE_
 * 前缀。Vite 自带的 envDir + envPrefix 那一套对不上（envPrefix 还不允许为空，否则
 * 等于把整个 process.env 全公开），所以仍然自己用 dotenv 读，再把**白名单内**的几个
 * 交给 Vite 的 config.env，src 里统一用 import.meta.env.X 取。见 injectClientEnv。
 *
 * （`.env` 在本项目是**目录**不是文件。Vite 自己找 .env 时会先 stat 一遍、只认
 *  isFile()，所以这个目录不会把 Vite 的 env 加载绊倒。）
 */
const readEnv = (
  file: string
): { env: Record<string, string>; error?: Error } => {
  const result = dotenv.config({ path: r(`.env/${file}`) });
  return { env: result.parsed ?? {}, error: result.error };
};

const commonEnv = readEnv(".env.common").env;

/** CSS Modules 的类名直通表：见 .env/.env.common 的 PASS_CLS */
const PASS_CLS: Record<string, "LK" | "EQ"> = JSON.parse(
  commonEnv.PASS_CLS ?? "{}"
);

/**
 * 复刻 webpack 里 css-loader 的 getLocalIdent。
 *
 * antd（ant-）、CodeMirror（cm-）、x-spreadsheet 这些第三方类名必须保持原样，否则
 * .module.scss 里对它们的覆盖会被哈希掉、静默失效。命中直通表就原样返回，其余按
 * webpack 原来的 `[local]--[hash:base64:5]` 生成。
 */
const generateScopedName = (name: string, filename: string): string => {
  for (const key in PASS_CLS) {
    if (PASS_CLS[key] === "LK" && name.startsWith(key)) return name;
    if (PASS_CLS[key] === "EQ" && name === key) return name;
  }

  const hash = createHash("md5")
    .update(`${path.relative(__dirname, filename)} ${name}`)
    .digest("base64")
    // base64 里的 +/= 不是合法的类名字符
    .replace(/[+/=]/g, "")
    .slice(0, 5);

  return `${name}--${hash}`;
};

/** 把 .env 里的 API_PROXY（JSON）翻成 Vite 的 server.proxy */
const buildProxy = (raw?: string): Record<string, ProxyOptions> => {
  if (!raw) return {};
  const parsed = JSON.parse(raw) as Record<
    string,
    {
      target: string;
      pathRewrite?: Record<string, string>;
      ReqHeader?: Record<string, string>;
    }
  >;

  const proxy: Record<string, ProxyOptions> = {};
  for (const [prefix, conf] of Object.entries(parsed)) {
    proxy[prefix] = {
      target: conf.target,
      changeOrigin: true,
      // 放行 WebSocket 升级请求（会话流走 WS）；对纯 HTTP 的代理没有副作用
      ws: true,
      // webpack-dev-server 的 pathRewrite 是「正则字符串 -> 替换值」，Vite 的 rewrite
      // 是个函数，这里逐条套用，语义保持一致
      rewrite: conf.pathRewrite
        ? (p) =>
            Object.entries(conf.pathRewrite!).reduce(
              (acc, [from, to]) => acc.replace(new RegExp(from), to),
              p
            )
        : undefined,
      configure: conf.ReqHeader
        ? (p) => {
            p.on("proxyReq", (proxyReq) => {
              for (const [k, v] of Object.entries(conf.ReqHeader!)) {
                proxyReq.setHeader(k, v);
              }
            });
          }
        : undefined,
    };
  }
  return proxy;
};

/**
 * 用 Babel 把**标准（Stage 3）装饰器 + 自动访问器**降级掉。
 *
 * 源码里是 mobx 6 的 `@observable accessor x = 1` 写法（stores/basisStoreClass 与
 * permit/User 共 19 处）。webpack 时期走 ts-loader，由 TypeScript 按 tsconfig 的
 * target(ES2020) 降级；Vite 的转译器不认这套，会把 `accessor` 原样输出，浏览器解析时
 * 直接报 `Unexpected identifier`、整页白屏——而且 build 能过，只在运行时炸。
 *
 * 只处理**确实含这套语法**的 .ts/.tsx，其余文件仍走默认转译，不为了几个文件拖慢全量。
 */
const lowerDecorators = (): Plugin => {
  const NEEDS =
    /(^|\s)accessor\s+[A-Za-z_$]|(^|\s)@[A-Za-z_$][\w$.]*\s*(\(|\r?\n|\s)/;

  return {
    name: "lower-standard-decorators",
    enforce: "pre",
    async transform(code, id) {
      if (!/\.tsx?$/.test(id) || id.includes("node_modules")) return null;
      if (!NEEDS.test(code)) return null;

      const babel = await import("@babel/core");
      const result = await babel.transformAsync(code, {
        filename: id,
        babelrc: false,
        configFile: false,
        sourceMaps: true,
        presets: [
          [
            "@babel/preset-typescript",
            { isTSX: id.endsWith(".tsx"), allExtensions: true },
          ],
        ],
        plugins: [
          ["@babel/plugin-proposal-decorators", { version: "2023-05" }],
          "@babel/plugin-transform-class-properties",
          // 装饰器降级会产出静态块，少了它 babel 直接报错
          "@babel/plugin-transform-class-static-block",
        ],
      });

      if (!result?.code) return null;
      return { code: result.code, map: result.map };
    },
  };
};

/**
 * x-data-spreadsheet 的 src/index.js 里有一句 `import './index.less'`，但它的样式本
 * 项目已经通过 @hsu-react/ui 引的 dist/xspreadsheet.css 拿到了。webpack 那边把这个
 * less 当字符串模块处理，这里同样把它短路成空样式，省掉一整套 less 工具链。
 */
const VIRTUAL_XS_LESS = "\0virtual:x-spreadsheet-less.css";

const stubSpreadsheetLess = (): Plugin => {
  // 按**路径**判断，不要按 importer：dev 下这个 less 是以
  // /node_modules/x-data-spreadsheet/src/index.less 这样的 URL 直接请求进来的，
  // 那一刻 importer 已经不是那个 src/index.js 了，挂在 importer 上会漏。
  const isSpreadsheetLess = (id: string) =>
    id.includes("x-data-spreadsheet") && id.endsWith(".less");

  return {
    name: "stub-x-data-spreadsheet-less",
    enforce: "pre",
    resolveId(source, importer) {
      if (isSpreadsheetLess(source)) return VIRTUAL_XS_LESS;
      // 相对写法（`./index.less`）在 resolveId 阶段还没拼成绝对路径，靠 importer 补
      if (
        source.endsWith(".less") &&
        importer &&
        isSpreadsheetLess(importer + source)
      ) {
        return VIRTUAL_XS_LESS;
      }
      if (source.endsWith(".less") && importer?.includes("x-data-spreadsheet")) {
        return VIRTUAL_XS_LESS;
      }
      return null;
    },
    load(id) {
      if (id === VIRTUAL_XS_LESS) return "";
      return null;
    },
  };
};

/**
 * **允许进入前端产物的环境变量白名单**，src 里通过 `import.meta.env.X` 读。
 *
 * 这是一道安全边界，不是图省事的清单：`config.env` 里的每一个键都会被**整体序列化**
 * 进 `import.meta.env`（dev 是模块头部的一行赋值，生产是内联字面量），所以只要写进去
 * 就等于公开。`.env` 里另外那几个是**纯构建期**用的，绝不能进来：
 *   - `API_PROXY` —— dev 代理目标（内网地址）；
 *   - `PASS_CLS`  —— CSS Modules 类名直通表；
 *   - `SERVER_PROT` —— dev 端口。
 *
 * `CRYPTO_KEY` / `RSA_PUB_KEY` 进来是**本来就该进**：前端要拿它们加密登录报文，
 * webpack 时期也一样打在产物里，不是这次新增的暴露。
 */
const CLIENT_ENV_KEYS = [
  "CRYPTO_KEY",
  "RSA_PUB_KEY",
  "API_BASE",
  "DEFAULT_PATH",
] as const;

/**
 * 把白名单里的值塞进 `config.env`，src 里就能用 `import.meta.env.X` 读到。
 *
 * **为什么不用 `define`（这次要修的就是它）**：Vite 8 里 `define` 对浏览器端**只在
 * 生产构建生效**。`vite:define` 插件的 `applyToEnvironment` 只有在环境
 * `isBundled` 时才把用户 define 交给打包器；dev 的 client 环境不打包，走的是插件
 * 本体的 transform，而那个 handler 第一行就是
 * `if (this.environment.config.consumer === "client") return;` —— 直接跳过。
 * 结果是 dev 下用户 define 一条都不替换（Vite 内置的 `process.env.NODE_ENV` 走的是
 * 另一条路，所以它看着是好的，很容易误判成「define 生效了」）。
 * 实测 8.2.2 与最新的 8.3.0 这段代码**一模一样**，是设计如此、不是某个版本的 bug，
 * 升版本解决不了。
 *
 * `import.meta.env` 则两边都通：dev 由 client 运行时注入成真对象，生产由打包器内联成
 * 字面量。两边同一个来源（`config.env`），不存在「dev 一套 prod 一套」。
 */
const injectClientEnv = (
  env: Record<string, string>,
  buildId: string
): Plugin => ({
  name: "inject-client-env",
  configResolved(config) {
    const clientEnv = config.env as Record<string, string>;
    for (const key of CLIENT_ENV_KEYS) {
      clientEnv[key] = env[key] ?? "";
    }
    clientEnv.BUILD_ID = buildId;
  },
});

/**
 * 产出 dist/build-id.txt。
 *
 * 页面运行时拿内嵌的 import.meta.env.BUILD_ID 跟这个文件比，不一致就强刷 ——
 * 根治 WKWebView（iOS 壳/客户端）拿旧缓存页的顽疾，见 src/utils/freshness.ts。
 */
const emitBuildId = (buildId: string): Plugin => ({
  name: "emit-build-id",
  generateBundle() {
    this.emitFile({ type: "asset", fileName: "build-id.txt", source: buildId });
  },
});

/**
 * 把 **antd 自己静态用到的那几十枚图标**并成一个 chunk。
 *
 * 起因：`@hsu-react/ui` 的 `Icon` 支持传 antd 图标名，2.5.12 起改成逐枚 `import()`
 * 按需加载（`@ant-design/icons/es/icons/<Name>`）。于是 antd 自己静态引着的那批
 * （`CloseOutlined`、`SearchOutlined`、`CaretDownFilled` …… 实测 39 枚）同时出现在
 * **静态图**和**动态图**里 —— rollup 的规矩是「能到达它的入口集合不同就切开」，
 * 每一枚 `import()` 目标又都是一个独立的 chunk 根，所以它们各自单独成块：
 * 首屏 script 54 → 93 个，gzip 896 → 919 KB。**raw 几乎没变（3119 → 3137 KB）**，
 * 多出来的全是碎片化的压缩损失 —— 同样的字节，分 39 个小文件各压一次。
 *
 * 这条规则只做一件事：把这 39 枚并回一个 chunk。判据是「除了 `@ant-design/icons`
 * 自己的 barrel 之外，还有别的静态引用方」——
 * 有，说明这一枚**本来就在首屏**（是 antd 的组件在用），并进来不会新增任何字节；
 * 没有，说明它只被那张按需表动态引到，**一枚都不碰**，继续保持「传到才下载」。
 *
 * 所以它不违反上面那条禁令：它不跨静态/动态边界，也不会把 846 枚拖成静态依赖
 * （真拖回去的话首屏会退回 1 MB，一眼能量出来）。
 */
const antdIconsStaticGroup = (
  id: string,
  { getModuleInfo }: { getModuleInfo: (id: string) => { importers: readonly string[] } | null }
): string | undefined => {
  if (!/@ant-design[\\/]icons[\\/]es[\\/]icons[\\/][A-Z][A-Za-z0-9]*\.js$/.test(id)) {
    return undefined;
  }
  const importers = getModuleInfo(id)?.importers ?? [];
  const fromOutside = importers.some(
    (imp) => !/@ant-design[\\/]icons[\\/]/.test(imp)
  );
  return fromOutside ? "antdIconsStatic" : undefined;
};

export default defineConfig(({ mode }): UserConfig => {
  const isProd = mode === "production";

  // .env.dev / .env.prod 都不入库（带本机代理目标与登录密钥），干净 clone 里没有。
  // dotenv 缺文件时 parsed 给的是 {} 不是 undefined，不拦一道的话构建照样成功、
  // 只是所有 process.env.* 被替换成 undefined，跑起来登录直接崩，而且崩在浏览器里、
  // 离构建现场十万八千里。宁可现在就编不过。
  const modeFile = isProd ? ".env.prod" : ".env.dev";
  const { env: modeEnv, error } = readEnv(modeFile);

  if (isProd) {
    const missing = ["NODE_ENV", "CRYPTO_KEY", "RSA_PUB_KEY"].filter(
      (k) => !modeEnv[k]
    );
    if (error || missing.length) {
      throw new Error(
        `.env/${modeFile} ${error ? "不存在或读不了" : `缺少 ${missing.join("、")}`}。先生成它：\n` +
          `  CRYPTO_KEY=… RSA_PUB_KEY=… bash scripts/write-env-prod.sh\n` +
          `（值必须与线上 hub 的 AM_CRYPTO_KEY / RSA 私钥配对，见 .env/.env.prod.example）`
      );
    }
  } else if (error) {
    throw new Error(
      `缺少 .env/${modeFile}。先 cp .env/.env.dev.example .env/.env.dev，` +
        `再按 docs/local-dev-setup.md 第一节把 CRYPTO_KEY / RSA_PUB_KEY 换成本机 hub 打印的值。`
    );
  }

  const env = { ...commonEnv, ...modeEnv };

  // 构建号只在生产有意义（dev 没有产物可比对），dev 一律给空串让检查自己跳过。
  const BUILD_ID = isProd ? Date.now().toString(36) : "";

  return {
    plugins: [
      injectClientEnv(env, BUILD_ID),
      lowerDecorators(),
      stubSpreadsheetLess(),
      react(),
      // vite 转译 TS 时不做类型检查；这一条补回 webpack 时期 fork-ts-checker 的作用。
      // 生产构建的类型检查由 package.json 的 `tsc --noEmit && vite build` 负责，
      // 这里只管 dev，不必跑两遍。
      checker({ typescript: true, enableBuild: false }),
      ...(isProd ? [emitBuildId(BUILD_ID)] : []),
    ],

    resolve: {
      alias: [
        { find: "@", replacement: r("src") },
        // webpack resolve.fallback 的等价物。node-forge 等库会 require 这几个 node
        // 内建，vite 不会自动补 polyfill，命中就会在预打包阶段直接报解析失败
        { find: /^crypto$/, replacement: "crypto-browserify" },
        { find: /^stream$/, replacement: "stream-browserify" },
        { find: /^vm$/, replacement: "vm-browserify" },
      ],
    },

    css: {
      modules: {
        generateScopedName,
      },
      preprocessorOptions: {
        scss: {
          additionalData: `@use "@/styles/variables.global.scss";`,
        },
      },
    },

    server: {
      port: Number(env.SERVER_PROT) || 3003,
      // 端口被占用时自动找下一个，对齐原来 portfinder 的行为
      strictPort: false,
      proxy: buildProxy(env.API_PROXY),
    },

    build: {
      outDir: "dist",
      emptyOutDir: true,
      sourcemap: false,
      rollupOptions: {
        output: {
          entryFileNames: "static/js/[name].[hash].js",
          // chunk 名不能以点开头。rolldown 给 CJS 互操作生成的内部模块就叫
          // `.esm-wrapper`（实测：acorn 经 FormCodeMirror 引入时产出
          // `.esm-wrapper.<hash>.chunk.js`），而任何把「点开头 = 隐藏文件」当规矩的
          // 工具都会静默丢掉它 —— GitHub Actions 的 upload-artifact 默认就丢，
          // 且丢完仍报成功，桌面客户端因此拿到过引用 404 的前端产物。
          // 在命名这一层削掉前导点，[hash] 保证削完仍不重名。
          chunkFileNames: (chunk) =>
            `static/js/${chunk.name.replace(/^\.+/, "") || "chunk"}.[hash].chunk.js`,
          assetFileNames: ({ names }) => {
            const ext = path
              .extname(names?.[0] ?? "")
              .slice(1)
              .toLowerCase();
            if (ext === "css") return "static/css/[name].[hash].css";
            return `static/media/${ext}/[name].[hash][extname]`;
          },
          // 这里**不要**写按包名分组的 manualChunks。
          //
          // webpack 那份配置的 splitChunks 就是按包名把三方库并成具名 chunk 的，代价是
          // 合并跨越「入口图」与「异步图」的边界：只要某个包在任何地方被引用，合并出来
          // 的 chunk 就会被算进首屏（改前 index.html 里挂着 95 个文件、5.15 MiB）。
          // rollup 默认的切分尊重静态/动态边界，交给它即可。
          //
          // 下面这条是**唯一的例外**，而且它刻意不跨那条边界 —— 只把「本来就已经是静态
          // 依赖」的模块并到一起，一个动态模块都不碰：
          manualChunks: antdIconsStaticGroup,
        },
      },
      chunkSizeWarningLimit: 1000,

      // 生产包里的日志要清掉（webpack 时期是 terser 的 drop_console / drop_debugger）。
      // Vite 默认的压缩器没有暴露这两个开关，所以显式切回 terser 并用同一组选项。
      minify: isProd ? "terser" : false,
      terserOptions: isProd
        ? { compress: { drop_console: true, drop_debugger: true } }
        : undefined,
    },
  };
});
