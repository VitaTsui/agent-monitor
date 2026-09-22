/**
 * 版本新鲜度守卫：对比打包内嵌 BUILD_ID 与服务端 /build-id.txt，
 * 不一致说明本地是缓存的旧页（WKWebView 壳内常见）→ 带缓存戳强刷。
 * 启动即查一次；回到前台（visibilitychange）再查，兜住长驻壳。
 */
import { reloadFor } from "./reloadTrace";

// BUILD_ID 由 vite.config.ts 的 injectClientEnv 注入：生产是构建号，dev 恒为空串 ——
// 下面 `if (!embedded)` 就直接跳过检查，dev 本来也没有产物可比对。
const embedded = import.meta.env.BUILD_ID;

let reloading = false;

async function check() {
  if (!embedded || reloading) {
    return;
  }
  try {
    const res = await fetch(`/build-id.txt?ts=${Date.now()}`, { cache: "no-store" });
    if (!res.ok) {
      return;
    }
    const server = (await res.text()).trim();
    if (server && server !== embedded) {
      reloading = true;
      const u = new URL(window.location.href);
      u.searchParams.set("v", server);
      reloadFor(`版本新鲜度守卫：页面构建 ${embedded} ≠ 服务器 ${server}`, () =>
        window.location.replace(u.toString()),
      );
    }
  } catch {
    // 网络异常时静默：新鲜度检查是增强，不能影响使用
  }
}

export function installFreshnessGuard() {
  void check();
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") {
      void check();
    }
  });
}
