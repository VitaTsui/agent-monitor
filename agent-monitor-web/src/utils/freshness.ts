/**
 * 版本新鲜度守卫：对比打包内嵌 BUILD_ID 与服务端 /build-id.txt，
 * 不一致说明本地是缓存的旧页（WKWebView 壳内常见）→ 带缓存戳强刷。
 * 启动即查一次；回到前台（visibilitychange）再查，兜住长驻壳。
 */
// prod 构建由 DefinePlugin 注入 process.env.BUILD_ID；dev 构建没有 `process`，
// 直接引用会在浏览器里抛 `process is not defined` 崩掉整个应用 —— 加 typeof 守卫。
const embedded =
  (typeof process !== "undefined" ? process.env.BUILD_ID : undefined) ?? "";

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
      window.location.replace(u.toString());
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
