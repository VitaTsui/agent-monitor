/**
 * 移动端底部抽屉弹窗「下滑收起」手势。
 *
 * 弹窗在窄屏是底部 sheet（见 styles/antd-overload.scss），移动端已隐藏右上角关闭钮，
 * 改为向下拖动关闭。全局委托，覆盖所有 antd/hsu Modal，无需逐个改。
 *
 * 起手条件（避免和内容滚动打架）：触点落在顶部抓手/标题区（<72px），或内容已滚到顶。
 * 释放时下移超过阈值（或快速下甩）→ 滑出并向 antd 发一次「取消」；否则弹回。
 */
import { isMobileViewport } from "./breakpoint";

/**
 * sheet 面板本体的类名。**必须与 styles/antd-overload.scss 里那份保持一致** ——
 * 手势和外观是同一套东西的两半，各写各的类名就会出现「样式是 sheet、手势却找不到面板」。
 *
 * 这里跟随 antd 当前大版本（v6：`.ant-modal-container`，见
 * node_modules/@rc-component/dialog/es/Dialog/Content/Panel.js 里 `${prefixCls}-container`；
 * v5 时叫 `.ant-modal-content`）。不写成「两个类名都试一遍」的兜底：那样 antd 再改一次
 * 就要叠第三个，而且旧类名失效时没有任何人会发现。
 */
const PANEL = ".ant-modal-container";
/** 收起 / 弹回的位移动画。 */
const SLIDE = "transform 0.25s cubic-bezier(0.32,0.72,0.2,1)";
/** 面板内的滚动区（antd v6 仍是 `-body`，同上 Panel.js）。 */
const BODY = ".ant-modal-body";

/**
 * 请求关闭最顶层弹窗。
 *
 * 走 antd 自己的 Esc 通道（@rc-component/portal 的 useEscKeyDown 在 window 上挂了
 * 原生 keydown，按栈只把最顶层那个交给 DialogWrap.onEsc → onClose），而不是去点
 * `.ant-modal-close`：
 * - `Modal.confirm` 一类命令式弹窗默认 `closable = false`（antd/es/modal/ConfirmDialog.js:139），
 *   根本不渲染关闭钮，点它等于什么都没做 —— 项目里有 6 处这种弹窗；
 * - `keyboard={false}` 的弹窗（如强制更新锁定框）会在这里被 antd 自己拒掉，
 *   手势因此不会绕过「必须更新才能继续」的设计。
 */
const requestClose = () => {
  window.dispatchEvent(
    new KeyboardEvent("keydown", { key: "Escape", bubbles: true }),
  );
};

/** 面板此刻还看得见吗（弹窗关掉后 antd 给 wrap 打内联 display:none，节点仍在）。 */
const stillVisible = (panel: HTMLElement) => {
  const wrap = panel.closest(".ant-modal-wrap") as HTMLElement | null;
  return (
    panel.isConnected && !!wrap && getComputedStyle(wrap).display !== "none"
  );
};

export function installSheetSwipe() {
  const isMobile = isMobileViewport;

  let active: HTMLElement | null = null;
  let startY = 0;
  let startT = 0;
  let dy = 0;

  const onStart = (e: TouchEvent) => {
    if (!isMobile() || e.touches.length !== 1) return;
    const target = e.target as HTMLElement;
    const panel = target.closest(PANEL) as HTMLElement | null;
    if (!panel) return;
    // 从可交互控件起手（输入框/按钮/可滚区中部）不接管，避免误触
    const y = e.touches[0].clientY;
    const rect = panel.getBoundingClientRect();
    const nearTop = y - rect.top < 72;
    const body = panel.querySelector(BODY) as HTMLElement | null;
    const atTop = !body || body.scrollTop <= 0;
    if (!nearTop && !atTop) return;
    active = panel;
    startY = y;
    startT = e.timeStamp;
    dy = 0;
  };

  const onMove = (e: TouchEvent) => {
    if (!active) return;
    const delta = e.touches[0].clientY - startY;
    if (delta <= 0) {
      // 上滑：若还没开始下拖，放弃接管交回滚动
      if (dy === 0) {
        active = null;
      }
      return;
    }
    dy = delta;
    // 确实在下拖：接管、阻止 body 滚动，跟手位移（带阻尼）
    e.preventDefault();
    const shift = dy < 0 ? 0 : dy * 0.9;
    active.style.transition = "none";
    active.style.transform = `translateY(${shift}px)`;
  };

  const springBack = (panel: HTMLElement) => {
    panel.style.transition = SLIDE;
    panel.style.transform = "translateY(0)";
    window.setTimeout(() => {
      panel.style.transition = "";
      panel.style.transform = "";
    }, 260);
  };

  const finish = (e: TouchEvent) => {
    if (!active) return;
    const panel = active;
    const dt = e.timeStamp - startT;
    const velocity = dy / Math.max(1, dt); // px/ms
    const shouldClose = dy > 130 || (dy > 60 && velocity > 0.5);
    active = null;
    dy = 0;

    if (!shouldClose) {
      springBack(panel);
      return;
    }

    // 先发取消再滑出：antd 的离场动画与这段位移同时跑，读起来都是「正在消失」，
    // 比等位移结束再关少 250ms 的延迟感。
    requestClose();
    panel.style.transition = SLIDE;
    panel.style.transform = "translateY(100%)";
    window.setTimeout(() => {
      if (stillVisible(panel)) {
        // 弹窗拒绝了关闭（keyboard={false} 的锁定框）—— 弹回去，别把它留在屏幕外
        springBack(panel);
        return;
      }
      // 复位，供该 DOM 复用（antd 默认不销毁，wrap 与面板会被下次打开复用）
      panel.style.transition = "";
      panel.style.transform = "";
    }, 300);
  };

  document.addEventListener("touchstart", onStart, { passive: true });
  document.addEventListener("touchmove", onMove, { passive: false });
  document.addEventListener("touchend", finish, { passive: true });
  document.addEventListener("touchcancel", finish, { passive: true });
}
