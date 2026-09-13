import { gcm } from "@noble/ciphers/aes";
import forge from "node-forge";

// 必须写成 `import.meta.env.XXX` 这种成员表达式：dev 下它是 client 运行时注入的真对象、
// 生产下由打包器内联成字面量，两边都认这个形状。解构成 `const { CRYPTO_KEY } = import.meta.env`
// 在生产下拿不到内联（打包器只认整段成员表达式），别改写法。
const DEF_KEY = import.meta.env.CRYPTO_KEY;
const PUB_KEY = import.meta.env.RSA_PUB_KEY;

const GCM_IV_LENGTH = 12;

/** 与 `AESUtils.buildKey` 一致：UTF-8 密钥长度须为 16 / 24 / 32 字节 */
function assertValidAesKeyUtf8(key: string): Uint8Array {
  const keyBytes = new TextEncoder().encode(key);
  const len = keyBytes.length;
  if (len !== 16 && len !== 24 && len !== 32) {
    throw new Error(
      `AES key length must be 16, 24, or 32 bytes (current=${len})`,
    );
  }
  return keyBytes;
}

function randomIv12(): Uint8Array {
  const iv = new Uint8Array(GCM_IV_LENGTH);
  if (typeof globalThis.crypto?.getRandomValues === "function") {
    globalThis.crypto.getRandomValues(iv);
    return iv;
  }
  const raw = forge.random.getBytesSync(GCM_IV_LENGTH);
  for (let i = 0; i < GCM_IV_LENGTH; i++) {
    iv[i] = raw.charCodeAt(i) & 0xff;
  }
  return iv;
}

function bytesToBinaryString(bytes: Uint8Array): string {
  let s = "";
  const chunk = 8192;
  for (let i = 0; i < bytes.length; i += chunk) {
    s += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return s;
}

/** 纯 JS（@noble/ciphers），与 Java `AES/GCM/NoPadding` 及包格式一致 */
function encryptAesJs(word: string, key: string): string {
  const keyBytes = assertValidAesKeyUtf8(key);
  const iv = randomIv12();
  const encrypted = gcm(keyBytes, iv).encrypt(new TextEncoder().encode(word));
  const payload = new Uint8Array(iv.length + encrypted.length);
  payload.set(iv, 0);
  payload.set(encrypted, iv.length);
  return btoa(bytesToBinaryString(payload));
}

function decryptAesJs(word: string, key: string): string {
  const keyBytes = assertValidAesKeyUtf8(key);
  const payload = Uint8Array.from(atob(word), (c) => c.charCodeAt(0));
  if (payload.length <= GCM_IV_LENGTH) {
    throw new Error("AES payload is invalid");
  }
  const iv = payload.slice(0, GCM_IV_LENGTH);
  const cipherText = payload.slice(GCM_IV_LENGTH);
  const plain = gcm(keyBytes, iv).decrypt(cipherText);
  return new TextDecoder().decode(plain);
}

/** 规范化为 X.509 SPKI PEM（支持 `.env` 里仅 base64、无头尾） */
function normalizeSpkiPem(key: string): string {
  const trimmed = key.trim();
  if (trimmed.includes("BEGIN PUBLIC KEY")) {
    return trimmed;
  }
  const b64 = trimmed.replace(/\s/g, "");
  const lines = b64.match(/.{1,64}/g) ?? [b64];
  return `-----BEGIN PUBLIC KEY-----\n${lines.join("\n")}\n-----END PUBLIC KEY-----`;
}

/**
 * 与 Java `Cipher.getInstance("RSA/ECB/OAEPWithSHA-256AndMGF1Padding")` 且在
 * `init()` 时**未**传入 `OAEPParameterSpec` 时的默认一致：
 * OAEP 消息摘要为 SHA-256，MGF1 仍为 **SHA-1**（OpenJDK 常见默认）。
 *
 * Web Crypto 的 RSA-OAEP 在指定 `hash: SHA-256` 时会把 MGF1 也固定为 SHA-256，
 * 与上述 JDK 默认不一致，会导致服务端 `BadPaddingException`。
 */
async function encodeRSA(data: string, key?: string): Promise<string> {
  if (!key) {
    key = PUB_KEY;
  }
  const publicKey = forge.pki.publicKeyFromPem(normalizeSpkiPem(key));
  const plain = forge.util.encodeUtf8(data);
  const encrypted = publicKey.encrypt(plain, "RSA-OAEP", {
    md: forge.md.sha256.create(),
    mgf1: { md: forge.md.sha1.create() },
  });
  return forge.util.encode64(encrypted);
}

// 解密方法（始终纯 JS，与后端 AESUtils 互通）
async function decrypt(word: string, key?: string): Promise<string> {
  if (!key) {
    key = DEF_KEY;
  }
  return decryptAesJs(word, key);
}

// 加密方法（始终纯 JS，与后端 AESUtils 互通）
async function encrypt(word: string, key?: string): Promise<string> {
  if (!key) {
    key = DEF_KEY;
  }
  return encryptAesJs(word, key);
}

export default {
  decrypt,
  encrypt,
  encodeRSA,
};
