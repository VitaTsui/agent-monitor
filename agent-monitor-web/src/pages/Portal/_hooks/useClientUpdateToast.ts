import { useEffect } from "react";

import { notification } from "@hsu-react/ui";

import { Button } from "@hsu-react/ui";
import React from "react";

import { inDesktopClient } from "@/utils/clientAuth";

interface UpdateStatus {
  current: string;
  latest: string | null;
}

/** 每个版本只弹一次（页面生命周期内） */
let toastedVersion: string | null = null;

/**
 * 客户端窗口内的新版本提醒：GUI 右下角弹一条可点击更新的通知。
 * 系统级通知对未签名应用经常被吞，这里在页面内实现，稳定可见；
 * 窗口隐藏时由托盘菜单与重新打开时的确认弹窗兜底。
 */
export function useClientUpdateToast() {
  useEffect(() => {
    if (!inDesktopClient()) {
      return;
    }
    const invoke = (
      window as unknown as {
        __TAURI__?: { core?: { invoke?: (cmd: string) => Promise<unknown> } };
      }
    ).__TAURI__?.core?.invoke;
    if (!invoke) {
      return;
    }

    let cancelled = false;
    const key = "client-update-toast";

    const check = () => {
      invoke("update_status")
        .then((v) => {
          if (cancelled) {
            return;
          }
          const s = v as UpdateStatus;
          if (!s.latest || toastedVersion === s.latest) {
            return;
          }
          toastedVersion = s.latest;
          notification.open({
            key,
            title: `新版本 v${s.latest} 可用`,
            description: `当前版本 v${s.current}，更新将自动完成并重启客户端。`,
            placement: "bottomRight",
            duration: 0,
            btn: React.createElement(
              Button,
              {
                type: "primary",
                size: "small",
                onClick: () => {
                  notification.destroy(key);
                  invoke("update_start").catch(() => void 0);
                },
              },
              "立即更新",
            ),
          });
        })
        .catch(() => void 0);
    };

    check();
    // 10s 轮询：与系统通知（update-watcher 3s）尽量同步，避免 GUI 提示晚一大截。
    // update_status 只是读内存里的 hub_latest_version，开销可忽略。
    const timer = window.setInterval(check, 10_000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);
}
