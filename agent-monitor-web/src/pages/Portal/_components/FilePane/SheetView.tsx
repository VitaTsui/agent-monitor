import React, { useEffect, useMemo, useState } from "react";

import { Icon, Table } from "@hsu-react/ui";
import type { ColumnsType } from "@hsu-react/ui";
// Segmented 走 antd：hsu-ui 没有这一枚，属 CLAUDE.md 里列的兜底项
import { Segmented } from "antd";

import styles from "./index.module.scss";

/**
 * 一张表最多**解析**多少行。只防解析，不防渲染。
 *
 * 渲染早已不是瓶颈：表格走组件库 `Table` 的虚拟滚动，只画看得见的那几十格，
 * 1069 行 × 44 列实测首屏 67ms（不虚拟时同一份要 10 秒、再大直接卡死浏览器）。
 * 此前这里是「200 行 × 20 列」的**渲染**上限，就是在替组件库那时失效的 `virtual`
 * 兜底 —— 结果一份 44 列的评分表后 24 列（总分所在）整个看不到。组件库修好后那条
 * 上限随之撤掉，列不再设限（列同样是虚拟的）。
 *
 * 剩下的开销是 `sheet_to_json` 把整张表读成数组，与行数成正比；这个数给得足够大，
 * 只挡「几十万行的导出文件」这种点开就要卡住解析的极端情况，超了照样在顶上明说。
 */
export const MAX_ROWS = 10000;

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
  /** 表名 → 合并单元格，见 `Spans` */
  spans: Record<string, Spans>;
  /** 有没有表被夹过 */
  clipped: boolean;
}

/**
 * 合并单元格：`"行,列"`（相对解析起点，与 `sheets` 的下标一致）→ 该格的跨度。
 *
 * 合并区左上角那一格给真实跨度；区内其余格给 `0 × 0`，antd 据此不画它们。
 * 不在表里的格子就是 1 × 1。表格的标题行、表头几乎都是合并出来的 —— 不还原的话，
 * 「2025 年……评审表」这种标题被挤在第一格里显示成省略号，多级表头也对不上下面的列。
 */
type Spans = Map<string, { rowSpan: number; colSpan: number }>;

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
        const spans: Record<string, Spans> = {};
        let clipped = false;
        wb.SheetNames.forEach((nm) => {
          const ws = wb.Sheets[nm];
          // 先把取值范围夹到上限再转：整张表转成 JSON 再切，十万行的表在切之前就卡死了
          let range: string | undefined;
          // 解析起点（表不一定从 A1 开始）：合并区的坐标是整张表的绝对位置，要换成相对它的
          let origin = { r: 0, c: 0 };
          let lastRow = Infinity;
          const ref = ws?.["!ref"];
          if (ref) {
            const r = XLSX.utils.decode_range(ref);
            origin = { r: r.s.r, c: r.s.c };
            if (r.e.r - r.s.r + 1 > MAX_ROWS) {
              r.e.r = r.s.r + MAX_ROWS - 1;
              clipped = true;
            }
            lastRow = r.e.r;
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
          cols[nm] = width;
          sheets[nm] = raw.map((r) =>
            Array.from({ length: cols[nm] }, (_, i) =>
              r[i] == null ? "" : String(r[i]),
            ),
          );
          const map: Spans = new Map();
          (ws?.["!merges"] ?? []).forEach((m) => {
            // 被行上限夹掉的部分不算：合并区整个在夹线以下就跳过，跨过夹线的截到夹线
            if (m.s.r > lastRow) {
              return;
            }
            const r0 = m.s.r - origin.r;
            const c0 = m.s.c - origin.c;
            const rows = Math.min(m.e.r, lastRow) - m.s.r + 1;
            const colsN = m.e.c - m.s.c + 1;
            for (let dr = 0; dr < rows; dr++) {
              for (let dc = 0; dc < colsN; dc++) {
                map.set(
                  `${r0 + dr},${c0 + dc}`,
                  dr === 0 && dc === 0
                    ? { rowSpan: rows, colSpan: colsN }
                    : { rowSpan: 0, colSpan: 0 },
                );
              }
            }
          });
          spans[nm] = map;
        });
        setParsed({ names: wb.SheetNames, sheets, cols, spans, clipped });
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
    const spans = parsed?.spans[active];
    // 列宽写死，不用 `autoWidth`：那条会为每一格量一次字宽，25000 格量下来卡的是主线程
    return Array.from({ length: n }, (_, i) => ({
      title: colLabel(i),
      dataIndex: `c${i}`,
      width: 128,
      // 按行号（`__k`）查，不按渲染下标：虚拟滚动下渲染下标不是数据下标
      onCell: (row: Row) => spans?.get(`${row.__k},${i}`) ?? {},
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
          表格太大，只显示前 {MAX_ROWS} 行
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
          /* 虚拟滚动：只画看得见的行与列，整张表（几千行、几十列）都能看全 */
          virtual
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
