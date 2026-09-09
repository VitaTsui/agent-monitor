/**
 * 主色与它的前景色 —— tokens.scss 里 `--primary` / `--primary-foreground` 的 JS 镜像。
 *
 * 为什么要有一份 JS 的：antd 要拿主色**派生一整条 10 级色板**，只能吃字面量，
 * 喂 `var(--primary)` 它算不出来。所以这两支必须与 tokens.scss 同步改动。
 *
 * 值完整照搬 shadcn 默认主题：浅色取中性阶最深的 zinc-900，暗色翻到最浅的 zinc-50，
 * 前景反过来取。
 *
 * **不要在任何组件里再写一次字面量。** 之前 Routes / Portal / 404 各写了一份
 * `#18181b`，后两份还是写死的 —— 暗色下主色翻成 zinc-50，那两处仍按墨黑走，
 * 于是同一颗按钮 CSS 变量给的是近白底、antd 给的是白字，字直接消失。
 */
export const primaryOf = (isDark: boolean): string =>
  isDark ? "#fafafa" : "#18181b";

export const primaryForegroundOf = (isDark: boolean): string =>
  isDark ? "#18181b" : "#fafafa";
