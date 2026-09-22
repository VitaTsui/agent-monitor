import React from "react";

import { Markdown } from "@hsu-react/ui";

type Components = NonNullable<React.ComponentProps<typeof Markdown.Views>["components"]>;
type AnchorProps = React.ComponentProps<"a"> & { node?: unknown };

/**
 * 内容里的链接（会话正文、历史记录、文件预览里的 Markdown）怎么打开。
 *
 * 组件库的 `Markdown.Views` 把链接渲染成普通 `<a href>`，点一下就在**当前窗口**里跳走：
 * 浏览器里整个门户被那个网页顶掉；桌面客户端的窗口没有地址栏也没有后退，界面被覆盖后
 * 回不来。内容里的链接从来不是「离开本应用」的意思，一律另开：
 *
 * - `http(s)://` 外链 → `target=_blank`。浏览器里是新标签页；桌面客户端里「开新窗口」
 *   请求由外壳交给系统浏览器（见 client 的 `on_new_window`）。
 * - `#锚点` → 照常在文内跳。
 * - 其余（`./tmp/x.md` 这类相对路径）→ 不跳。它指的是跑 agent 那台机器上的文件，
 *   在本站打开只会把整个界面导航到一个不存在的页面，与外链是同一个「覆盖界面」。
 *
 * App 自己的跳转（钉钉 / 第三方登录要在当前窗口走完授权再回来）不经过这里，不受影响 ——
 * 这也是规则放在「内容链接」这一层、而不是在客户端一刀切拦掉所有离站跳转的原因。
 */
function ContentLink({ node: _node, href, children, ...rest }: AnchorProps) {
  if (href && /^https?:\/\//i.test(href)) {
    return (
      <a {...rest} href={href} target="_blank" rel="noopener noreferrer">
        {children}
      </a>
    );
  }
  if (href?.startsWith("#")) {
    return (
      <a {...rest} href={href}>
        {children}
      </a>
    );
  }
  return (
    <a
      {...rest}
      href={href}
      title={href}
      onClick={(e) => {
        e.preventDefault();
      }}
    >
      {children}
    </a>
  );
}

/** 渲染内容类 Markdown 时传给 `Markdown.Views` 的 `components` */
export const contentMarkdownComponents: Components = { a: ContentLink };
