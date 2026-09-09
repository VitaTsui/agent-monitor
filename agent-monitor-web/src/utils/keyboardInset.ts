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
  // 仅触摸设备（有软键盘）才接管：桌面客户端/桌面浏览器的 webview 也有 visualViewport，
  // 之前无差别接管会在桌面客户端里改 #root 尺寸/触发 reflow，把首个键入字符吞掉
  // （表现为「输入 / 得连按两次才显示」）。桌面无软键盘，直接不接管。
  const isTouch =
    "ontouchstart" in window || (navigator.maxTouchPoints ?? 0) > 0;
  const vv = window.visualViewport;
  const rootEl = document.getElementById("root");
  if (vv && rootEl && !inCapacitor && isTouch) {
    let lastH = 0;
    const apply = () => {
      // 只在高度真变化时改 #root（键盘弹起/收起）；不监听 scroll、不做 transform，
      // 避免打字过程中频繁 reflow 吞掉输入。键盘弹起 → #root 缩到可视视口高 → 内容与
      // 对话框整体压到键盘上方。
      const h = Math.round(vv.height);
      if (h === lastH) return;
      lastH = h;
      rootEl.style.height = `${h}px`;
    };
    vv.addEventListener("resize", apply);
    window.addEventListener("orientationchange", () =>
      window.setTimeout(() => {
        lastH = 0;
        apply();
      }, 300),
    );
    apply();
  }
}
