import { useEffect } from "react";

import { notification } from "antd";

import { getVersionInfo } from "@/services/apis/portal";

/** Capacitor 注入的桥（远程壳：插件 JS 不在本包，走全局对象） */
interface CapacitorBridge {
  isNativePlatform?: () => boolean;
  Plugins?: {
    App?: { getInfo?: () => Promise<{ version?: string }> };
  };
}

/** a 是否比 b 更新（点分数字逐段比较，与服务端 version_newer 同规则） */
function newer(a: string, b: string): boolean {
  const pa = a.split(".").map((x) => parseInt(x, 10) || 0);
  const pb = b.split(".").map((x) => parseInt(x, 10) || 0);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const x = pa[i] ?? 0;
    const y = pb[i] ?? 0;
    if (x !== y) return x > y;
  }
  return false;
}

/**
 * 移动端更新推送：仅在 Capacitor 原生壳内生效。
 *
 * 远程壳的网页本身随站点发布自动更新；这里管的是 APK 壳自身——
 * 用 @capacitor/app 取本机版本，与 hub 的 /monitor/version（android 字段，
 * 打包时写进 downloads/manifest.json）比对，有新版弹通知带下载链接。
 * 浏览器里没有 Capacitor 全局，钩子空转。
 */
export function useApkUpdateCheck() {
  useEffect(() => {
    const cap = (window as unknown as { Capacitor?: CapacitorBridge }).Capacitor;
    if (!cap?.isNativePlatform?.()) {
      return;
    }
    const getInfo = cap.Plugins?.App?.getInfo;
    if (!getInfo) {
      return;
    }

    let cancelled = false;
    Promise.all([getInfo(), getVersionInfo()])
      .then(([info, res]) => {
        if (cancelled || res.code !== 0) {
          return;
        }
        const latest = res.data?.android;
        const current = info?.version;
        if (!latest || !current || !newer(latest, current)) {
          return;
        }
        const dl = `${process.env.API_BASE ?? ""}/downloads/${encodeURIComponent(
          "终端任务监控-android.apk",
        )}`;
        notification.info({
          message: `新版 App v${latest} 可用`,
          description: "点击下载安装包，安装后覆盖当前版本即可。",
          duration: 0,
          onClick: () => window.open(dl, "_blank"),
        });
      })
      .catch(() => {
        // 检测失败静默：更新提示是锦上添花，不该打扰使用
      });

    return () => {
      cancelled = true;
    };
  }, []);
}
