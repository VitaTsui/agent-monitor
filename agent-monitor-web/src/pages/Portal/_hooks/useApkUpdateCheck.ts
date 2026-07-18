import { useEffect } from "react";

import { Modal } from "antd";

import { getVersionInfo } from "@/services/apis/portal";

/** Capacitor 注入的桥（远程壳：插件 JS 不在本包，走全局对象） */
interface CapacitorBridge {
  isNativePlatform?: () => boolean;
  Plugins?: {
    App?: {
      getInfo?: () => Promise<{ version?: string }>;
      exitApp?: () => Promise<void>;
    };
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

/** APK 直链：英文 + 版本号命名，稳定别名兜底 */
const apkUrl = (version?: string | null) =>
  `${process.env.API_BASE ?? ""}/downloads/${
    version ? `AgentMonitor-${version}.apk` : "AgentMonitor.apk"
  }`;

/** 强制更新弹窗：只有「去更新」，点了也不关（必须装新版才能继续用） */
function showForcedModal(latest: string, cap: CapacitorBridge) {
  const url = apkUrl(latest);
  Modal.confirm({
    title: `必须更新到 v${latest}`,
    content:
      "当前 App 版本已停止支持，必须更新后才能继续使用。点击「去更新」下载安装包，安装后重新打开；选择退出将关闭应用。",
    okText: "去更新",
    cancelText: "退出应用",
    closable: false,
    maskClosable: false,
    keyboard: false,
    onOk: () => {
      window.open(url, "_blank");
      // 返回被拒绝的 Promise：弹窗保持打开，App 维持锁定状态
      return Promise.reject(new Error("keep-open"));
    },
    onCancel: () => {
      const exit = cap.Plugins?.App?.exitApp;
      if (exit) {
        void exit();
      } else {
        // 退不掉（个别壳版本无 exitApp）就重新弹出，保持锁定
        showForcedModal(latest, cap);
      }
    },
  });
}

/**
 * 移动端更新推送：仅在 Capacitor 原生壳内生效。
 *
 * 远程壳的网页本身随站点发布自动更新；这里管的是 APK 壳自身：
 * - 有新版：弹确认框，用户点「去更新」下载安装包；
 * - 低于强制更新下限（/monitor/version 的 androidMin）：锁定弹窗，
 *   必须更新才能继续使用，拒绝则退出应用。
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
        const minVer = res.data?.androidMin;
        const current = info?.version;
        if (!current) {
          return;
        }
        // 强制更新优先：低于下限就锁定
        if (minVer && newer(minVer, current)) {
          showForcedModal(latest ?? minVer, cap);
          return;
        }
        if (latest && newer(latest, current)) {
          Modal.confirm({
            title: `发现新版 App v${latest}`,
            content: "是否立即下载更新？安装后覆盖当前版本即可。",
            okText: "去更新",
            cancelText: "稍后",
            onOk: () => {
              window.open(apkUrl(latest), "_blank");
            },
          });
        }
      })
      .catch(() => {
        // 检测失败静默：更新提示是锦上添花，不该打扰使用
      });

    return () => {
      cancelled = true;
    };
  }, []);
}
