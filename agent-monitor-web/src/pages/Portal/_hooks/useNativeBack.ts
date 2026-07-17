import { useEffect, useRef } from "react";

/**
 * Capacitor 注入到 WebView 里的桥。
 * 本项目的移动端是「远程壳」——原生工程直接加载线上站点，所以插件的 JS 侧
 * 不在本包里，只能通过桥的全局对象取用。浏览器里没有这个全局，钩子自动空转。
 */
interface CapacitorBridge {
  isNativePlatform?: () => boolean;
  Plugins?: {
    App?: {
      addListener: (
        event: "backButton",
        cb: (data: { canGoBack?: boolean }) => void,
      ) => Promise<{ remove: () => void }> | { remove: () => void };
      exitApp?: () => void;
    };
  };
}

/**
 * 接管 Android 的返回键。
 *
 * 不接管的话，返回键会直接 finish Activity（整个 App 退出）——抽屉开着、
 * 弹窗开着都一样，用户按返回就没了。
 *
 * @param onBack 返回 true 表示「本次返回已被消费」（例如关掉了抽屉），
 *               返回 false 则交还系统语义：能后退就后退，否则退出 App。
 */
export function useNativeBack(onBack: () => boolean) {
  // 用 ref 存回调：监听器只注册一次，但回调要能读到最新的状态
  const handlerRef = useRef(onBack);
  handlerRef.current = onBack;

  useEffect(() => {
    const cap = (window as unknown as { Capacitor?: CapacitorBridge }).Capacitor;
    const app = cap?.Plugins?.App;
    if (!app?.addListener) {
      // 浏览器 / 非原生环境：什么都不做
      return;
    }

    let remove: (() => void) | null = null;
    let disposed = false;

    const result = app.addListener("backButton", ({ canGoBack }) => {
      if (handlerRef.current()) {
        return;
      }
      if (canGoBack) {
        window.history.back();
      } else {
        app.exitApp?.();
      }
    });

    // addListener 在不同版本里可能同步返回句柄，也可能返回 Promise
    Promise.resolve(result).then((handle) => {
      if (disposed) {
        handle?.remove();
        return;
      }
      remove = () => handle?.remove();
    });

    return () => {
      disposed = true;
      remove?.();
    };
  }, []);
}
