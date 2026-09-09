import React, { useEffect, useState } from "react";

import { Markdown } from "@hsu-react/ui";

import {
  SessionImageCtx,
  resolveSessionImages,
} from "@/utils/sessionImages";

interface Props {
  content: string;
  /** 会话上下文：本地图片路径以它的 cwd 为根解析；缺省则不解析，退化成普通 Markdown */
  imageCtx?: SessionImageCtx;
}

/**
 * 会话内容的 Markdown 渲染：与 `Markdown.Views` 唯一的差别是**会把本地图片路径解析出来**。
 *
 * agent 输出里的 `![说明](qa/evidence/x.png)` 指的是跑 agent 那台机器上的文件，
 * 直接交给浏览器只会 404（见 utils/sessionImages）。
 *
 * 先渲染原文再异步替换：取图要一趟 IPC/网络，先挂着不显示会让整段内容闪一下 ——
 * 而绝大多数消息根本不含图片，为它们等一轮不值当。
 */
const SessionMarkdown: React.FC<Props> = ({ content, imageCtx }) => {
  const [resolved, setResolved] = useState(content);

  useEffect(() => {
    setResolved(content);
    let alive = true;
    resolveSessionImages(content, imageCtx).then((out) => {
      // 无图时 resolveSessionImages 返回的就是原字符串，这里不会触发多余渲染
      if (alive && out !== content) {
        setResolved(out);
      }
    });
    return () => {
      alive = false;
    };
  }, [content, imageCtx]);

  return <Markdown.Views>{resolved}</Markdown.Views>;
};

export default SessionMarkdown;
