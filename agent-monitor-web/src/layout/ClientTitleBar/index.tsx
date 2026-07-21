import React, { useEffect, useState } from "react";

import { inDesktopClient } from "@/utils/clientAuth";
import styles from "./index.module.scss";

/** 仅在 macOS 客户端启用：那里窗口用 Overlay 标题栏（原生交通灯 + 内容延伸到顶部）。
 *  Windows/Linux 保持系统原生边框，不需要自绘顶栏。 */
const isMac = () =>
  /Mac|iPhone|iPad/i.test(navigator.platform) ||
  /Mac OS X/i.test(navigator.userAgent);

/**
 * macOS 无边框融合窗口的顶部拖拽条（对标 Claude / Codex 桌面端）：
 * - 只在 macOS 客户端窗口内渲染；
 * - 一整条透明 data-tauri-drag-region 叠在最上方，供拖动窗口（Overlay 去掉了系统标题栏，
 *   顶部默认不可拖）；
 * - 原生交通灯由 macOS 提供，落在左上；页面顶部让出标题栏高度避免内容与交通灯重叠
 *   （见全局 desktop-client 样式）。
 */
const ClientTitleBar: React.FC = () => {
  const [show, setShow] = useState(false);
  useEffect(() => {
    const on = inDesktopClient() && isMac();
    setShow(on);
    if (on) {
      document.documentElement.classList.add("desktop-client");
      document.documentElement.classList.add("is-mac");
    }
  }, []);

  if (!show) {
    return null;
  }
  return <div className={styles.bar} data-tauri-drag-region />;
};

export default ClientTitleBar;
