/**
 * 移动端启动图收尾：网页首屏渲染好后主动关掉原生 SplashScreen —— 快加载
 * 立刻关（不等固定时长），慢加载由原生 launchShowDuration 兜底。启动图
 * 一直盖着到内容就绪，消除「启动图消失→页面未加载」的白屏。
 * 浏览器里无 Capacitor 桥，静默跳过。
 */
export function hideSplashWhenReady() {
  const cap = (window as unknown as {
    Capacitor?: { isNativePlatform?: () => boolean; Plugins?: { SplashScreen?: { hide?: (o?: unknown) => void } } };
  }).Capacitor;
  const hide = cap?.Plugins?.SplashScreen?.hide;
  if (!cap?.isNativePlatform?.() || !hide) {
    return;
  }
  // 首屏挂载 + 两帧后（让首屏真正绘制出来）再关，避免关早了仍闪白
  requestAnimationFrame(() =>
    requestAnimationFrame(() => {
      setTimeout(() => hide({ fadeOutDuration: 200 }), 60);
    }),
  );
}
