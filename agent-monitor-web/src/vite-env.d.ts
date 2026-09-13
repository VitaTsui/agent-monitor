/// <reference types="vite/client" />

/**
 * 由 `vite.config.ts` 的 `injectClientEnv` 注入（白名单 `CLIENT_ENV_KEYS` + BUILD_ID），
 * 值来自 `.env/.env.common` 与 `.env/.env.dev|prod`。
 *
 * 声明成 `string` 有个实打实的好处：键名写错（`import.meta.env.API_BSAE`）`tsc` 当场
 * 报错。以前用 `process.env.X` 时写错只会静默拿到 `undefined`，得等线上出问题才发现。
 */
interface ImportMetaEnv {
  /** 登录报文 AES 密钥，须与 hub 的 AM_CRYPTO_KEY 配对 */
  readonly CRYPTO_KEY: string;
  /** 登录报文 RSA 公钥（SPKI/DER/base64），须与 hub 的 RSA 私钥配对 */
  readonly RSA_PUB_KEY: string;
  /** 接口前缀；dev 下是走 vite 代理的 `/api`，生产下同源为空串 */
  readonly API_BASE: string;
  /** 登录后的默认落地路由 */
  readonly DEFAULT_PATH: string;
  /** 生产构建号，与 dist/build-id.txt 比对做强刷；dev 下为空串 */
  readonly BUILD_ID: string;
}
