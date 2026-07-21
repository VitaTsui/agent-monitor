/**
 * iOS 键盘处理：目标是「背景页面整体不动，只把对话框和内容推上去」。
 *
 * - Capacitor 原生壳：监听 keyboardWillShow/Hide，把键盘高度写进 --kb-height，
 *   底部据此加内边距（webview 不缩放、页面不动）。
 * - 浏览器 / iOS Safari（无 Capacitor 事件）：用 visualViewport 把 #root 的高度钉成
 *   「可视视口高度」。键盘弹起时可视视口变矮 → #root 跟着变矮 → 内容与底部对话框
 *   整体压缩到键盘上方；因为可视区里已经没有多余高度可滚，iOS 便不会再把整页顶上去。
 */
export function installKeyboardInset() {
  const root = document.documentElement;
  const set = (h: number) =>
    root.style.setProperty("--kb-height", `${Math.max(0, Math.round(h))}px`);
  set(0);

  const readHeight = (e: Event): number => {
    const anyE = e as unknown as {
      keyboardHeight?: number;
      detail?: { keyboardHeight?: number };
    };
    return anyE.keyboardHeight ?? anyE.detail?.keyboardHeight ?? 0;
  };

  const inCapacitor = !!(window as unknown as { Capacitor?: unknown }).Capacitor;

  // 原生壳：走键盘事件 + --kb-height 内边距
  window.addEventListener("keyboardWillShow", (e) => set(readHeight(e)));
  window.addEventListener("keyboardDidShow", (e) => set(readHeight(e)));
  window.addEventListener("keyboardWillHide", () => set(0));
  window.addEventListener("keyboardDidHide", () => set(0));

  // 浏览器：把 #root 高度钉成可视视口高（仅非原生壳，避免与原生 resize 打架）
  const vv = window.visualViewport;
  const rootEl = document.getElementById("root");
  if (vv && rootEl && !inCapacitor) {
    const apply = () => {
      // 可视视口被键盘/地址栏顶偏移时，连带把 #root 往下挪同样距离，视觉上背景不动
      rootEl.style.height = `${Math.round(vv.height)}px`;
      rootEl.style.transform =
        vv.offsetTop > 0 ? `translateY(${Math.round(vv.offsetTop)}px)` : "";
      // 顺手把布局视口滚回顶部，杜绝 iOS 残留的整页滚动
      if (window.scrollY !== 0) window.scrollTo(0, 0);
    };
    vv.addEventListener("resize", apply);
    vv.addEventListener("scroll", apply);
    window.addEventListener("orientationchange", () =>
      window.setTimeout(apply, 300),
    );
    apply();
  }
}
