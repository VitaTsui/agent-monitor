/**
 * iOS 键盘内边距：配合 Keyboard 插件 resize:none（webview 不缩放、页面不动），
 * 监听键盘高度写进 CSS 变量 --kb-height。页面底部据此加内边距，让底部的
 * 输入框升到键盘上方、内容区收缩，而顶栏/整页不位移 —— ChatGPT/Claude iOS 式。
 *
 * Capacitor Keyboard 会在 window 上派发 keyboardWillShow/Hide 事件，
 * 键盘高度在 event.keyboardHeight 或 event.detail.keyboardHeight。
 * 浏览器里无这些事件，变量恒为 0，无副作用。
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

  window.addEventListener("keyboardWillShow", (e) => set(readHeight(e)));
  window.addEventListener("keyboardDidShow", (e) => set(readHeight(e)));
  window.addEventListener("keyboardWillHide", () => set(0));
  window.addEventListener("keyboardDidHide", () => set(0));

  // 浏览器 / iOS Safari（无 Capacitor 事件）：用 visualViewport 侦测键盘。
  // iOS 弹键盘时布局视口(innerHeight)不变、可视视口(visualViewport.height)缩小，
  // 差值即键盘高度。据此把 --kb-height 写上去，底部对话框升到键盘上方、内容收缩，
  // 输入框始终可见 → iOS 不必再把整页顶上去（页面不动，只有对话框与内容被推上去）。
  const vv = window.visualViewport;
  if (vv) {
    // 记录不含键盘的布局高度（随旋转/窗口变化更新，但不被键盘的可视视口缩放污染）
    let layoutH = window.innerHeight;
    window.addEventListener("orientationchange", () => {
      // 旋转后等布局稳定再取
      setTimeout(() => {
        layoutH = window.innerHeight;
      }, 300);
    });
    const update = () => {
      // 取当前 innerHeight 与记录布局高的较大者，避免个别 iOS 版本 innerHeight
      // 也随键盘缩小时把键盘高算成 0
      const base = Math.max(window.innerHeight, layoutH);
      // 键盘高度 = 布局视口 - 可视视口。不减 offsetTop：那是可视视口的滚动量，
      // 减掉会低算键盘高、把对话框顶进键盘里，形成 iOS 继续上顶的反馈环。
      const kb = Math.max(0, Math.round(base - vv.height));
      // < 100 视为非键盘的细微抖动（地址栏收放等），归 0
      set(kb > 100 ? kb : 0);
    };
    vv.addEventListener("resize", update);
    vv.addEventListener("scroll", update);
    update();
  }
}
