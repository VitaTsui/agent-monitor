import hljs from "highlight.js/lib/core";
import bash from "highlight.js/lib/languages/bash";
import css from "highlight.js/lib/languages/css";
import diff from "highlight.js/lib/languages/diff";
import go from "highlight.js/lib/languages/go";
import ini from "highlight.js/lib/languages/ini";
import java from "highlight.js/lib/languages/java";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import markdown from "highlight.js/lib/languages/markdown";
import plaintext from "highlight.js/lib/languages/plaintext";
import python from "highlight.js/lib/languages/python";
import rust from "highlight.js/lib/languages/rust";
import scss from "highlight.js/lib/languages/scss";
import sql from "highlight.js/lib/languages/sql";
import typescript from "highlight.js/lib/languages/typescript";
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";

/**
 * 文件查看器的语法高亮。
 *
 * 走 `highlight.js/lib/core` ＋ **按需注册**，不是整包：整包 386 个语言定义会整个进
 * 首屏，而这儿只认下面这十几种（本项目与它监控的项目实际会打开的那些）。
 * 形制照 VitaAgent `components/FileViewer/highlight.ts` —— 连「不套 `.hljs` 外层类」
 * 这一条也照抄：那个类自带 atom-one-dark 的深底，而我们的源码态是跟着主题走的
 * 面色（见 index.module.scss）。
 */
const LANGS: Record<string, Parameters<typeof hljs.registerLanguage>[1]> = {
  bash,
  css,
  diff,
  go,
  ini,
  java,
  javascript,
  json,
  markdown,
  plaintext,
  python,
  rust,
  scss,
  sql,
  typescript,
  xml,
  yaml,
};

/** 别名：扩展名给的名字不一定等于 highlight.js 的语言名 */
const ALIASES: Record<string, string[]> = {
  xml: ["html", "htm", "svg", "vue"],
  typescript: ["ts", "tsx", "jsx"],
  javascript: ["js", "mjs", "cjs"],
  bash: ["sh", "zsh", "shell"],
  python: ["py"],
  rust: ["rs"],
  yaml: ["yml"],
  ini: ["toml", "conf", "cfg", "env", "properties"],
  markdown: ["md", "mdx"],
  plaintext: ["text", "txt", "log"],
};

let registered = false;
const ensureRegistered = () => {
  if (registered) {
    return;
  }
  registered = true;
  Object.entries(LANGS).forEach(([name, def]) =>
    hljs.registerLanguage(name, def),
  );
  Object.entries(ALIASES).forEach(([languageName, aliases]) =>
    hljs.registerAliases(aliases, { languageName }),
  );
};

const escapeHtml = (s: string) =>
  s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

/**
 * 把一段高亮后的 HTML 按行切开，每一行都**自洽**。
 *
 * highlight.js 的 `<span>` 可以跨行（多行注释、多行字符串），直接按 `\n` 切会把标签
 * 切断。这里扫一遍、记住开着的 span 栈：遇到换行先把开着的都关上，下一行开头再原样
 * 打开。输出里只有带 class 的 span 与已转义的文本，不需要完整的 HTML 解析器。
 * 逐字照 VitaAgent `components/FileViewer/highlight.ts:71-94`。
 */
export const splitHighlightedLines = (html: string): string[] => {
  const lines: string[] = [];
  const stack: string[] = [];
  let cur = "";
  let last = 0;
  const re = /<span class="([^"]*)">|<\/span>|\n/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(html))) {
    cur += html.slice(last, m.index);
    last = re.lastIndex;
    if (m[0] === "\n") {
      lines.push(cur + "</span>".repeat(stack.length));
      cur = stack.map((c) => `<span class="${c}">`).join("");
    } else if (m[0] === "</span>") {
      stack.pop();
      cur += m[0];
    } else {
      stack.push(m[1]);
      cur += m[0];
    }
  }
  cur += html.slice(last);
  lines.push(cur);
  return lines;
};

/**
 * 一段文本 → 每行一段 HTML。
 *
 * 认不出的语言**不猜**（不走 `highlightAuto`）：猜错的高亮比没有高亮更难读，
 * 而且自动识别要把所有注册过的语言都跑一遍。认不出就原样转义。
 */
export const highlightLines = (text: string, lang: string): string[] => {
  ensureRegistered();
  const name = lang.toLowerCase();
  if (!name || !hljs.getLanguage(name)) {
    return splitHighlightedLines(escapeHtml(text));
  }
  try {
    const out = hljs.highlight(text, { language: name, ignoreIllegals: true });
    return splitHighlightedLines(out.value);
  } catch {
    // 高亮失败不该让整个查看器空掉
    return splitHighlightedLines(escapeHtml(text));
  }
};
