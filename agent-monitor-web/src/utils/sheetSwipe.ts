/**
 * 移动端底部抽屉弹窗「下滑收起」手势。
 *
 * 弹窗在窄屏是底部 sheet（见 styles/antd-overload.scss），移动端已隐藏右上角关闭钮，
 * 改为向下拖动关闭。全局委托，覆盖所有 antd/hsu Modal，无需逐个改。
 *
 * 起手条件（避免和内容滚动打架）：触点落在顶部抓手/标题区（<72px），或内容已滚到顶。
 * 释放时下移超过阈值（或快速下甩）→ 滑出并 click 隐藏的关闭钮触发 onCancel；否则弹回。
 */
export function installSheetSwipe() {
  const isMobile = () => window.matchMedia("(max-width: 760px)").matches;

  let active: HTMLElement | null = null;
  let startY = 0;
  let startT = 0;
  let dy = 0;

  const bodyOf = (content: HTMLElement) =>
    content.querySelector(".ant-modal-body") as HTMLElement | null;

  const onStart = (e: TouchEvent) => {
    if (!isMobile() || e.touches.length !== 1) return;
    const target = e.target as HTMLElement;
    const content = target.closest(".ant-modal-content") as HTMLElement | null;
    if (!content) return;
    // 从可交互控件起手（输入框/按钮/可滚区中部）不接管，避免误触
    const y = e.touches[0].clientY;
    const rect = content.getBoundingClientRect();
    const nearTop = y - rect.top < 72;
    const body = bodyOf(content);
    const atTop = !body || body.scrollTop <= 0;
    if (!nearTop && !atTop) return;
    active = content;
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

  const finish = (e: TouchEvent) => {
    if (!active) return;
    const content = active;
    const dt = e.timeStamp - startT;
    const velocity = dy / Math.max(1, dt); // px/ms
    active = null;
    content.style.transition = "transform 0.25s cubic-bezier(0.32,0.72,0.2,1)";
    const shouldClose = dy > 130 || (dy > 60 && velocity > 0.5);
    if (shouldClose) {
      content.style.transform = "translateY(100%)";
      const close = content
        .closest(".ant-modal")
        ?.querySelector(".ant-modal-close") as HTMLElement | null;
      window.setTimeout(() => {
        close?.click();
        // 复位，供该 DOM 复用（antd 复用 wrap）
        content.style.transform = "";
        content.style.transition = "";
      }, 200);
    } else {
      content.style.transform = "translateY(0)";
      window.setTimeout(() => {
        content.style.transition = "";
        content.style.transform = "";
      }, 260);
    }
    dy = 0;
  };

  document.addEventListener("touchstart", onStart, { passive: true });
  document.addEventListener("touchmove", onMove, { passive: false });
  document.addEventListener("touchend", finish, { passive: true });
  document.addEventListener("touchcancel", finish, { passive: true });
}
