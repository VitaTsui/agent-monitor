import React, { useEffect, useState } from "react";

import { inDesktopClient } from "@/utils/clientAuth";
import styles from "./index.module.scss";

/** 是否 macOS（Overlay 标题栏保留原生交通灯，只需拖拽条；Windows/Linux 需自绘按钮） */
const isMac = () =>
  /Mac|iPhone|iPad/i.test(navigator.platform) ||
  /Mac OS X/i.test(navigator.userAgent);

const invoke = (cmd: string) => {
  const fn = (
    window as unknown as {
      __TAURI__?: { core?: { invoke?: (c: string) => Promise<unknown> } };
    }
  ).__TAURI__?.core?.invoke;
  return fn ? fn(cmd).catch(() => void 0) : Promise.resolve();
};

/**
 * 无边框窗口的自绘顶栏（对标 ChatGPT / Claude Code 客户端）：
 * - 只在桌面客户端窗口内渲染（浏览器里不显示）；
 * - 一整条透明拖拽区（data-tauri-drag-region）叠在内容最上方；
 * - macOS：靠左给原生交通灯留位，右侧纯拖拽；
 * - Windows/Linux：右上角自绘最小化 / 关闭按钮（走 IPC）。
 * 顶栏本身透明，视觉上与下方内容融为一体；只负责「能拖动 + 能最小化/关闭」。
 */
const ClientTitleBar: React.FC = () => {
  const [show, setShow] = useState(false);
  useEffect(() => {
    const on = inDesktopClient();
    setShow(on);
    // 给 <html> 打标记：客户端里各页面顶部让出标题栏高度（见全局样式）
    if (on) {
      document.documentElement.classList.add("desktop-client");
      document.documentElement.classList.add(isMac() ? "is-mac" : "is-win");
    }
  }, []);

  if (!show) {
    return null;
  }
  const mac = isMac();
  return (
    <div className={`${styles.bar} ${mac ? styles.mac : styles.win}`}>
      {/* 拖拽区单独成元素：按钮不能是 data-tauri-drag-region 的子孙，否则在 Windows
          WebView2 上按下即被判为拖窗，onClick 永不触发（表现为按钮点了没反应） */}
      <div className={styles.dragArea} data-tauri-drag-region />
      {!mac && (
        <div className={styles.controls}>
          <span
            className={styles.ctrl}
            role="button"
            aria-label="最小化"
            title="最小化"
            onClick={() => invoke("win_minimize")}
          >
            <svg width="11" height="11" viewBox="0 0 11 11" aria-hidden>
              <rect x="1" y="5" width="9" height="1.2" fill="currentColor" />
            </svg>
          </span>
          <span
            className={`${styles.ctrl} ${styles.close}`}
            role="button"
            aria-label="关闭"
            title="关闭"
            onClick={() => invoke("win_close")}
          >
            <svg width="11" height="11" viewBox="0 0 11 11" aria-hidden>
              <path
                d="M1 1 L10 10 M10 1 L1 10"
                stroke="currentColor"
                strokeWidth="1.2"
                fill="none"
              />
            </svg>
          </span>
        </div>
      )}
    </div>
  );
};

export default ClientTitleBar;
