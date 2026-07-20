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
}
