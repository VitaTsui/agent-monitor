/**
 * 会话名字：显示用哪一个、输入怎么算长度。
 *
 * 集中一处的理由与 portalNav 相同：同一个「这条会话叫什么」在侧栏、会话头部、移动端顶栏、
 * 列表排序四个地方各写了一遍。备注要排在回退链最前，四处漏一处就会出现「列表改了名、
 * 顶栏还是旧标题」。
 */

import { PortalTaskData } from "@/services/apis/portal";

/**
 * 备注长度上限（**字符**数，中文一个字算一个）。与 hub 的 notes::NOTE_MAX_CHARS 一致。
 *
 * 前端也要有这个数：超了就在输入框里当场说，而不是让用户点了保存再吃一个 400。
 * 后端仍是唯一裁决方 —— 它的 400 照样正常展示。
 */
export const NOTE_MAX_CHARS = 100;

/**
 * 与 hub 的 notes::normalize 同规则的归一化：控制字符（含换行制表符）折成空格、
 * 连续空白折一个、首尾去空。
 *
 * 前端做这一遍只为**算长度**和判空：用户粘进来一段带换行的文本时，字数要按后端最终
 * 存下的那份算，否则输入框说 98 个字、后端却回 400。真正落库的归一化仍由后端做。
 */
export const normalizeNote = (raw: string): string =>
  [...raw]
    // 控制字符先换成空格，再连同普通空白一起折叠 —— 与后端逐字同序
    .map((c) => {
      const code = c.codePointAt(0) ?? 0;
      return code < 0x20 || code === 0x7f ? " " : c;
    })
    .join("")
    .replace(/\s+/g, " ")
    .trim();

/** 归一化后的字符数（用展开而非 .length，免得表情符号被按 UTF-16 码元算成两个） */
export const noteLength = (raw: string): number => [...normalizeNote(raw)].length;

/**
 * 这条会话显示什么名字。备注在最前 —— 它是用户亲手起的，比任何自动推断都准。
 *
 * @param fallback 什么都没有时的兜底文案（各处不同：侧栏「新会话」、顶栏站名等）
 */
export const sessionTitle = (t: PortalTaskData, fallback: string): string =>
  t.note || t.title || t.prompt || t.projectName || fallback;
