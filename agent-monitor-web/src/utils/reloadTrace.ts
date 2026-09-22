/**
 * 页面重载溯源：每一次由我方代码发起的刷新都先留下原因，下一次页面起来时把
 * 「这次是怎么加载的 + 上一页留下的原因」写进客户端日志（client.log）。
 *
 * 为什么要有它：用户反映「界面时不时直接刷新掉」，而会发起刷新的路径有好几条
 * （版本新鲜度守卫、401 静默续登、托盘「刷新」、配对完成、后管令牌失效），
 * 事后只看代码分不出是哪一条。有了这份记录，下次刷新一查日志就知道；
 * 若日志里是「重载、原因无记录」，说明根本不是网页代码发起的（WebView 进程
 * 被系统回收后自动重载之类），该往客户端/系统那一层去查。
 *
 * 原因放 sessionStorage：同一个窗口的下一次加载读得到，读完即删，不跨窗口串味。
 * 本模块不依赖请求层，Axios 拦截器也能直接用它而不形成循环引用。
 */

const KEY = "am_reload_reason";

interface Reason {
  reason: string;
  at: number;
}

interface TauriBridge {
  core?: {
    invoke?: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
  };
}

/** 写一行到客户端日志；浏览器里（或客户端版本太旧没有该接口）只打控制台 */
function clientLog(msg: string) {
  const invoke = (window as unknown as { __TAURI__?: TauriBridge }).__TAURI__
    ?.core?.invoke;
  console.info(`[reload-trace] ${msg}`);
  invoke?.("page_log", { msg }).catch(() => void 0);
}

/** 记下原因再执行跳转（默认原地重载）。所有「我方主动刷新」都必须走这里。 */
export function reloadFor(
  reason: string,
  go: () => void = () => window.location.reload(),
) {
  try {
    sessionStorage.setItem(KEY, JSON.stringify({ reason, at: Date.now() } satisfies Reason));
  } catch {
    // 存不进去也照样刷新：诊断记录不能挡住功能
  }
  go();
}

/** 页面启动时调用一次：这次加载若是重载、或上一页留了原因，就记一笔 */
export function traceBoot() {
  let prev: Reason | null = null;
  try {
    const raw = sessionStorage.getItem(KEY);
    sessionStorage.removeItem(KEY);
    prev = raw ? (JSON.parse(raw) as Reason) : null;
  } catch {
    prev = null;
  }
  const nav = performance.getEntriesByType("navigation")[0] as
    | PerformanceNavigationTiming
    | undefined;
  const type = nav?.type ?? "unknown";
  // 正常打开（首次进入、点链接跳转）且没有留原因：不是刷新，不记
  if (!prev && type === "navigate") {
    return;
  }
  const why = prev
    ? `${prev.reason}（${Math.round((Date.now() - prev.at) / 1000)} 秒前发起）`
    : "无记录 —— 不是网页代码发起的（WebView 自行重载 / 系统 / 手动）";
  clientLog(`页面加载 type=${type} path=${window.location.pathname} 原因=${why}`);
}
