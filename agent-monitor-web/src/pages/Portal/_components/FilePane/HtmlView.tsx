import React, { useMemo } from "react";

import styles from "./index.module.scss";

/**
 * HTML 来自远端机器，不能和主页面共享执行环境。
 *
 * `sandbox` 负责隔离脚本、表单和导航；CSP 再把外网资源堵住，避免预览一个 HTML 就向
 * 文件作者指定的地址发请求。内联样式、data/blob 图片仍可用，静态页面能正常还原。
 */
const PREVIEW_CSP = [
  "default-src 'none'",
  "style-src 'unsafe-inline'",
  "img-src data: blob:",
  "font-src data:",
  "media-src data: blob:",
].join("; ");

const isolateHtml = (source: string): string => {
  const doc = new DOMParser().parseFromString(source, "text/html");

  // sandbox 已经不会执行这些节点；主动删掉，既减少无意义的报错，也不给后续放宽
  // sandbox 时留下一个隐蔽的执行入口。
  doc
    .querySelectorAll("script, base, object, embed, iframe")
    .forEach((node) => node.remove());
  doc
    .querySelectorAll('meta[http-equiv="refresh" i]')
    .forEach((node) => node.remove());

  const policy = doc.createElement("meta");
  policy.httpEquiv = "Content-Security-Policy";
  policy.content = PREVIEW_CSP;
  doc.head.prepend(policy);

  return `<!doctype html>${doc.documentElement.outerHTML}`;
};

interface HtmlViewProps {
  source: string;
  title: string;
}

const HtmlView: React.FC<HtmlViewProps> = ({ source, title }) => {
  const srcDoc = useMemo(() => isolateHtml(source), [source]);

  return (
    <iframe
      className={styles.htmlPreview}
      title={title}
      sandbox=""
      referrerPolicy="no-referrer"
      srcDoc={srcDoc}
    />
  );
};

export default HtmlView;
