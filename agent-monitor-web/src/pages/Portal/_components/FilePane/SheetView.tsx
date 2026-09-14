import React, { useEffect, useMemo, useState } from "react";

import { Icon, Table } from "@hsu-react/ui";
import type { ColumnsType } from "@hsu-react/ui";
// Segmented 走 antd：hsu-ui 没有这一枚，属 CLAUDE.md 里列的兜底项
import { Segmented } from "antd";

import styles from "./index.module.scss";

/**
 * 一次最多画多少格。与源码态的 `MAX_LINES` 是同一套表达：**超了就夹掉、并在顶上明说**。
 *
 * **这两个数是量出来的，不是拍的**：组件库的 `Table` 不做虚拟滚动，每一格都是真的
 * DOM 节点，代价随格数线性涨。同一台机器、同一份 dev 构建实测（从点开到表体出行
 * ／随后一帧的耗时）：
 *
 *   |   格数 | 首次出行 | 每帧   |
 *   |-------:|---------:|-------:|
 *   |  2 121 |   541 ms | 122 ms |
 *   |  3 926 |   872 ms | 232 ms |
 *   | 12 341 |  2767 ms | 1242 ms|
 *   | 25 551 |  5706 ms | 2759 ms|
 *
 * 25 000 格那一档，页面整整**冻住五秒多**、之后每帧还要两秒半 —— 截图工具都拿不到
 * 一帧。200 × 20 = 4000 格落在「首次出行不到一秒、帧不到 250 毫秒」这一档里。
 *
 * 这一列实测最宽 441px、一列 128px，一屏也就看得见三四列 —— 再多给的不是信息，是卡顿。
 */
export const MAX_ROWS = 200;
export const MAX_COLS = 20;

/** 列名照电子表格的习惯：0 → A、25 → Z、26 → AA。**不拿第一行当表头** —— 表里有没有表头只有人知道 */
const colLabel = (i: number): string => {
  let n = i;
  let out = "";
  do {
    out = String.fromCharCode(65 + (n % 26)) + out;
    n = Math.floor(n / 26) - 1;
  } while (n >= 0);
  return out;
};

interface Parsed {
  names: string[];
  /** 表名 → 已夹过的二维字符串。单元格一律是字符串，渲染时当文本用 */
  sheets: Record<string, string[][]>;
  /** 最宽的那张表有多少列 */
  cols: Record<string, number>;
  /** 有没有表被夹过 */
  clipped: boolean;
}

/** 一行：`__k` 是 rowKey，`c0`/`c1`… 是各列 */
type Row = Record<string, string | number>;

interface SheetViewProps {
  /** 原始字节。xlsx/xls 是二进制，csv/tsv 也走同一条解析路径 */
  bytes: Uint8Array<ArrayBuffer>;
  /**
   * 这份文件本身是不是文本（csv / tsv 是，xlsx / xls 不是）。
   *
   * **它决定把字节还是把字符串交给解析器**，不是可有可无的提示：SheetJS 收到字节
   * 时会自己猜 CSV 的代码页，默认按 latin1 认——中文 UTF-8 直接变成
   * 「è®¾å¤ å」那种乱码（实测）。文本的编码这边已经知道了（UTF-8），
   * 自己解完再给它，就没有猜的余地。
   */
  textual: boolean;
}

/**
 * 表格态：xlsx / xls / csv / tsv 画成表。
 *
 * **解析库是动态 `import()` 进来的**，不是顶层 import：`xlsx` 压缩后约 400 KB，
 * 而绝大多数会话从头到尾不会打开一个表格文件 —— 让它进主 chunk 等于所有人替这个
 * 功能付首屏。这样它自成一个 chunk，只有真的点开表格时才下载。
 *
 * 单元格内容来自**别人机器上的文件**，等同不可信输入：这里一律交给组件库 `Table`
 * 当 React 子节点渲染（`dataSource` 里每一格都是 `String(...)` 出来的纯字符串），
 * React 对文本节点自动转义，全程没有 `dangerouslySetInnerHTML` ——
 * `<img onerror=...>` 这种单元格只会原样显示成一串字符。
 *
 * TODO(agent-monitor): npm 上的 `xlsx` 停在 0.18.5（SheetJS 已迁出 npm，改由官方源
 * cdn.sheetjs.com 发布），该版本有已知的原型污染（官方 0.19.3 修）与 ReDoS
 * （0.20.2 修），**触发方式正是解析恶意构造的表格文件** —— 也就是这个组件干的事。
 * 正确的修法是把依赖源换到 SheetJS 官方源并升到 ≥0.20.2，不是在这儿加判断。
 */
const SheetView: React.FC<SheetViewProps> = ({ bytes, textual }) => {
  const [parsed, setParsed] = useState<Parsed | null>(null);
  /** 解析没成时的原文。空 = 没失败 */
  const [fail, setFail] = useState("");
  const [active, setActive] = useState("");

  useEffect(() => {
    let alive = true;
    setParsed(null);
    setFail("");
    import("xlsx")
      .then((XLSX) => {
        if (!alive) {
          return;
        }
        const wb = textual
          ? XLSX.read(new TextDecoder("utf-8").decode(bytes), { type: "string" })
          : XLSX.read(bytes, { type: "array" });
        const sheets: Record<string, string[][]> = {};
        const cols: Record<string, number> = {};
        let clipped = false;
        wb.SheetNames.forEach((nm) => {
          const ws = wb.Sheets[nm];
          // 先把取值范围夹到上限再转：整张表转成 JSON 再切，十万行的表在切之前就卡死了
          let range: string | undefined;
          const ref = ws?.["!ref"];
          if (ref) {
            const r = XLSX.utils.decode_range(ref);
            if (r.e.r - r.s.r + 1 > MAX_ROWS) {
              r.e.r = r.s.r + MAX_ROWS - 1;
              clipped = true;
            }
            if (r.e.c - r.s.c + 1 > MAX_COLS) {
              r.e.c = r.s.c + MAX_COLS - 1;
              clipped = true;
            }
            range = XLSX.utils.encode_range(r);
          }
          const raw = XLSX.utils.sheet_to_json<unknown[]>(ws ?? {}, {
            header: 1,
            // `raw: false` 要的是**显示值**：日期、百分比、货币都按表里的格式串出来，
            // 拿原始序列号给人看等于没显示
            raw: false,
            defval: "",
            range,
          });
          const width = raw.reduce((m, r) => Math.max(m, r.length), 0);
          cols[nm] = Math.min(width, MAX_COLS);
          sheets[nm] = raw.map((r) =>
            Array.from({ length: cols[nm] }, (_, i) =>
              r[i] == null ? "" : String(r[i]),
            ),
          );
        });
        setParsed({ names: wb.SheetNames, sheets, cols, clipped });
        setActive(wb.SheetNames[0] ?? "");
      })
      .catch((e: unknown) => {
        if (alive) {
          // 解析不了是**真的一种结果**（文件损坏、加了密），照实说，不静默给一张空表
          setFail(e instanceof Error ? e.message : "这份表格解析不了");
        }
      });
    return () => {
      alive = false;
    };
  }, [bytes, textual]);

  const columns = useMemo<ColumnsType<Row>>(() => {
    const n = parsed ? (parsed.cols[active] ?? 0) : 0;
    // 列宽写死，不用 `autoWidth`：那条会为每一格量一次字宽，25000 格量下来卡的是主线程
    return Array.from({ length: n }, (_, i) => ({
      title: colLabel(i),
      dataIndex: `c${i}`,
      width: 128,
    }));
  }, [parsed, active]);

  const dataSource = useMemo<Row[]>(() => {
    const rows = parsed?.sheets[active] ?? [];
    return rows.map((r, i) => {
      const row: Row = { __k: i };
      r.forEach((cell, c) => {
        row[`c${c}`] = cell;
      });
      return row;
    });
  }, [parsed, active]);

  if (fail) {
    return (
      <div className={styles.empty}>
        <div>{fail}</div>
      </div>
    );
  }
  if (!parsed) {
    return (
      <div className={styles.hint}>
        <Icon icon="ph:table" className={styles.hintIcon} />
        <span>正在解析表格…</span>
      </div>
    );
  }
  if (!parsed.names.length) {
    return <div className={styles.empty}>这份表格里没有工作表</div>;
  }

  return (
    <>
      {parsed.clipped ? (
        <div className={styles.clip}>
          表格太大，只显示前 {MAX_ROWS} 行 × {MAX_COLS} 列
        </div>
      ) : null}
      {/* 只有一张表就不摆切换器：一颗点不出第二个去处的按钮是噪声 */}
      {parsed.names.length > 1 ? (
        <div className={styles.sheetTabs}>
          <Segmented
            size="small"
            value={active}
            onChange={(v) => setActive(String(v))}
            options={parsed.names}
          />
        </div>
      ) : null}
      <div className={styles.sheet}>
        <Table<Row>
          size="small"
          rowKey="__k"
          columns={columns}
          dataSource={dataSource}
          pagination={false}
          /* 行号列由组件库给（`serialNumberColumn`），不自己糊一列 ——
             它与项目里所有列表的「序号」列是同一副长相 */
          serialNumberColumn
          /* 表头钉住、表体自己滚（横向 ＋ 纵向都在表里）。
             组件库的 `.Table` 是 `height: 100%` 的 flex 列，**父级必须有确定高度**，
             所以外面那层 `.sheet` 被摆进了一个 flex 的 `.body`（见 index.tsx 的
             `bodyFill`）—— 不给定高的话实测表体高度算出来是 0，一行都画不出来。
             `scrollAutoHeight` 要关：它会把表高写成「表头＋表体的自然高度」，
             把 `height: 100%` 顶掉，长表又变成外层滚。 */
          scroll
          scrollAutoHeight={false}
          /* 只读查看器不给排序：点一下就把行序打乱，而人是照着文件里的顺序找东西的 */
          sorter={false}
        />
      </div>
    </>
  );
};

export default SheetView;
