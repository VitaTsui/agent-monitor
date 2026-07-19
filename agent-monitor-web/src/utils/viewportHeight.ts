/**
 * iOS 键盘处理：用 visualViewport 把「可视高度」写进 CSS 变量 --app-vh，
 * 键盘弹出时可视区从底部收缩、顶部不动 —— 配合 Keyboard 插件 resize:none，
 * 实现 ChatGPT/Claude iOS 那样「顶栏固定、只输入框跟随键盘上移」。
 *
 * 浏览器里没有键盘顶起问题，visualViewport 也存在，逻辑一致无副作用。
 */
export function installViewportHeightVar() {
  const vv = window.visualViewport;
  const root = document.documentElement;

  const apply = () => {
    const h = vv ? vv.height : window.innerHeight;
    root.style.setProperty("--app-vh", `${Math.round(h)}px`);
    // 键盘高度（可视底部到布局视口底部的距离），供输入区做额外留白兜底
    const kb = vv ? Math.max(0, window.innerHeight - vv.height - vv.offsetTop) : 0;
    root.style.setProperty("--kb-inset", `${Math.round(kb)}px`);
  };

  apply();
  if (vv) {
    vv.addEventListener("resize", apply);
    vv.addEventListener("scroll", apply);
  }
  window.addEventListener("orientationchange", () => setTimeout(apply, 250));
}
