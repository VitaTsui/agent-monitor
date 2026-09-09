import { useEffect, useRef, useState } from "react";

/**
 * 拆分视图的自适应网格：按容器**实际宽高**决定横向还是纵向拆分、一行摆几个。
 *
 * 原先是写死的（1 格单列、2 格两列、3/4 格 2×2），只看格子数不看容器：竖屏或窄
 * 窗口下两格并排会各自挤成一条细缝，而超宽屏上四格挤成 2×2 又白白浪费横向空间。
 * 这里改成枚举所有列数、按「每格摊到的尺寸」打分选最优。
 */

/**
 * 单格最小可用宽/高：低于此值消息内容就挤得没法读，宁可少分几列。
 *
 * 宽度阈值给到 400 是因为会话里常有代码块和表格 —— 横向空间比纵向金贵得多，
 * 窄一点就开始折行。实测 320 太松：1440 宽的笔记本上会选出「4 列 × 359px」
 * 这种细长条，比 2×2 难用得多。
 */
const MIN_PANE_W = 400;
const MIN_PANE_H = 240;

/**
 * 单格的理想宽高比（w / h）。略宽于高（1.1）而不是竖长：对话虽然竖向流动，但
 * 头部和输入框吃掉的是固定高度，真正稀缺的是行宽。
 */
const TARGET_RATIO = 1.1;

/**
 * 违反最小尺寸的惩罚权重。取值远大于比例项的量级（比例项是 log 距离，通常 < 2），
 * 保证「每格还能读」永远压过「比例好看」。
 */
const PENALTY = 10;

/**
 * 末行排不满的代价（如 4 格分 3 列 = 3 + 1 通栏）。这种布局上下格子不一样宽，
 * 看着是歪的 —— 光比比例的话它常常险胜规整解（1440×820 下 3×2 就压过了 2×2），
 * 所以显式记一笔。取 0.5：比例上明显更优时仍可胜出（超宽屏 4 格照样排成一行），
 * 势均力敌时让规整的赢。
 */
const UNEVEN_PENALTY = 0.5;

export interface PaneGrid {
  cols: number;
  rows: number;
  /** 末行没排满时，最后一格要跨几列才能填满整行；排满则为 1 */
  lastSpan: number;
}

/**
 * 在 1..n 列之间挑最优解。
 *
 * 打分 = 比例偏离 + 尺寸不足惩罚，取最小者。比例偏离用 log 距离，这样「宽扁 2 倍」
 * 和「瘦高 2 倍」受罚相同——用差值或比值都会偏袒其中一侧。
 */
export const solvePaneGrid = (
  w: number,
  h: number,
  n: number,
  gap: number
): PaneGrid => {
  if (n <= 1 || w <= 0 || h <= 0) {
    return { cols: 1, rows: Math.max(n, 1), lastSpan: 1 };
  }

  let best: PaneGrid = { cols: 1, rows: n, lastSpan: 1 };
  let bestCost = Infinity;

  for (let cols = 1; cols <= n; cols += 1) {
    const rows = Math.ceil(n / cols);
    const pw = (w - gap * (cols - 1)) / cols;
    const ph = (h - gap * (rows - 1)) / rows;
    if (pw <= 0 || ph <= 0) {
      continue;
    }

    let cost = Math.abs(Math.log(pw / ph / TARGET_RATIO));
    // 按「差多少」线性加罚，而不是一票否决：容器小到怎么排都不达标时，
    // 仍能挑出相对最不憋屈的那个，而不是退化成固定值。
    if (pw < MIN_PANE_W) {
      cost += PENALTY * (1 - pw / MIN_PANE_W);
    }
    if (ph < MIN_PANE_H) {
      cost += PENALTY * (1 - ph / MIN_PANE_H);
    }
    if (n % cols !== 0) {
      cost += UNEVEN_PENALTY;
    }

    if (cost < bestCost) {
      bestCost = cost;
      const last = n % cols || cols;
      best = { cols, rows, lastSpan: cols - last + 1 };
    }
  }

  return best;
};

/**
 * 观测容器尺寸并算出网格。返回的 ref 挂到网格容器上。
 *
 * @param count 当前打开的会话数
 * @param gap   网格间距，需与样式里的 gap 一致（差 1px 不影响选择结果，只影响边界判定）
 */
export const usePaneGrid = (count: number, gap = 1) => {
  const ref = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });

  useEffect(() => {
    const el = ref.current;
    if (!el) {
      return;
    }

    const ro = new ResizeObserver((entries) => {
      const rect = entries[0]?.contentRect;
      if (!rect) {
        return;
      }
      const { width, height } = rect;
      // 取整再比对：亚像素抖动（滚动条出现/消失、缩放）否则会每帧 setState
      const w = Math.round(width);
      const h = Math.round(height);
      setSize((prev) => (prev.w === w && prev.h === h ? prev : { w, h }));
    });
    ro.observe(el);

    return () => ro.disconnect();
    // count 变化时容器可能刚挂载（0 格时不渲染网格），必须重新绑定
  }, [count]);

  return { ref, ...solvePaneGrid(size.w, size.h, count, gap) };
};
