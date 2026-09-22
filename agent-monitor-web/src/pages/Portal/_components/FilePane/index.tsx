import React, { useCallback, useEffect, useRef, useState } from "react";

import classNames from "classnames";

import { Icon, Markdown } from "@hsu-react/ui";
import { Tooltip, message } from "antd";

import { getTaskDirs, getTaskFile, hasListing } from "@/services/apis/portal";
import CodeLines from "./CodeLines";
import SheetView from "./SheetView";
import styles from "./index.module.scss";

/**
 * 目录与文件的取件节奏（毫秒）。总时长约 92 秒。
 *
 * **这条阶梯是被往返时间逼出来的，不是拍脑袋**：目录/文件都由那台机器在**下一轮上报**
 * 时带回，实测上报约 31 秒一轮、刚错过一轮就是 62 秒（`Composer/index.tsx:70-78`
 * 记着同一份实测，那儿的目录选择器用的就是这套值）。前几拍密集是照顾「缓存已热、
 * 秒回」的情况，随后退避，避免长时间空转刷请求。
 */
const POLL_DELAYS = [
  1200, 1200, 1500, 2000, 3000, 4000, 5000, 6000, 8000, 10000, 12000, 15000,
  22000,
];

/** 一次最多渲染多少行。源码按行渲染，一个几万行的文件能把这一列卡住 */
const MAX_LINES = 5000;

/** 扩展名 → 高亮语言名（`highlight.ts` 里注册过别名，这里给原始扩展名即可） */
const extOf = (name: string): string => {
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
};

/** 这份内容能不能当文本读：**自己判，不看后端给的 MIME** —— 理由见组件注释 */
const looksText = (bytes: Uint8Array<ArrayBuffer>): boolean => {
  const n = Math.min(bytes.length, 4096);
  if (!n) {
    return true;
  }
  let bad = 0;
  for (let i = 0; i < n; i += 1) {
    const b = bytes[i];
    // NUL 基本只出现在二进制里，见一个就够
    if (b === 0) {
      return false;
    }
    // 可打印 ASCII ＋ 常见空白之外的控制字符算「坏字节」；UTF-8 的高位字节不算
    if (b < 9 || (b > 13 && b < 32)) {
      bad += 1;
    }
  }
  return bad / n < 0.05;
};

/** 位图。**svg 不在里面** —— 它是文本，自成一类（有预览 / 源码两态，见 `Kind`） */
const IMAGE_EXT = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "ico"];

const MD_EXT = ["md", "markdown"];

/** 表格。前两个是二进制（没有源码态），后两个本身就是文本（有） */
const SHEET_EXT = ["xlsx", "xls", "csv", "tsv"];

/** base64 → 字节。`atob` 一次性拿到二进制字符串，再逐字节转（文件上限 10 MB） */
const b64ToBytes = (b64: string): Uint8Array<ArrayBuffer> => {
  const bin = atob(b64);
  const buf = new ArrayBuffer(bin.length);
  const out = new Uint8Array(buf);
  for (let i = 0; i < bin.length; i += 1) {
    out[i] = bin.charCodeAt(i);
  }
  return out;
};

/**
 * 这份文件**按什么方式画**。扩的是这一个枚举，不是在 `text` 旁边再挂开关：
 * 「是不是 markdown」这件事只该有一个判据，两套判断并存迟早对不上。
 *
 * 其中 `markdown` / `svg` / `sheet` 有**渲染态与源码态两态**（见 `hasTwoViews`）：
 * 默认渲染态，头部那颗按钮切到源码。`text` 只有源码一态，`image` / `pdf` /
 * `binary` 连源码都没有。
 */
type Kind =
  | "markdown"
  | "sheet"
  | "svg"
  | "text"
  | "image"
  | "pdf"
  | "binary";

/**
 * 一次取件为什么没成。**三类分得开，靠的是结构化信号，不是文案**：
 *
 * - `deny`：服务端在信封里给了非 0 的 `code`（hub 的 `err()`：HTTP 恒 200，真正的状态
 *   码在 body 的 `code` 上）。这是**明确的拒绝** —— 比如「该设备是他人共享给你的，
 *   不提供文件访问」「该文件被安全策略拒绝」。重试一万次结果一样，所以**不给重试**。
 * - `timeout`：取件阶梯（`POLL_DELAYS`，约 92 秒）走完，那台机器仍没把内容带回来。
 *   下一轮上报就可能回来，**给重试**。
 * - `network`：请求本身没打到 hub。也给重试。
 *
 * 从前这一层只有一个字符串：目录那一支把 `code !== 0` 直接当成超时、把 `msg` 丢了，
 * 文件那一支用 `"timeout"` 当哨兵值混在真实文案里。两个毛病同一个根：
 * **失败原因没有类型，只有一句话**。
 */
type FailKind = "deny" | "timeout" | "network";

interface Fail {
  kind: FailKind;
  /** `deny` 时是服务端原文；另两类前端自己说 */
  msg: string;
}

const timeoutFail = (what: "目录" | "文件"): Fail => ({
  kind: "timeout",
  msg: `这台机器还没把${what}送回来（一轮上报约 30 秒，最坏 60 秒）`,
});

const NETWORK_FAIL: Fail = {
  kind: "network",
  msg: "读取失败，请检查网络",
};

interface FileState {
  rel: string;
  name: string;
  kind: Kind;
  text?: string;
  /** 图片 / PDF 的 objectURL，切走时要 revoke */
  url?: string;
  size: number;
  /** 行数超过 `MAX_LINES` 被夹过 */
  clipped?: boolean;
  /** 原始字节，只有 `sheet` 留 —— 表格要交给解析器，文本那份没法回推 */
  bytes?: Uint8Array<ArrayBuffer>;
}

/**
 * 这份文件有没有「渲染 ↔ 源码」两态。
 *
 * 判据只有两条、都来自 `FileState` 本身：**这一类天生有渲染态**，并且
 * **手里确实有源码可给**。`.csv` 有（它就是文本），`.xlsx` 没有（二进制，
 * 给不出源码，所以那颗按钮根本不出现）。
 */
const hasTwoViews = (f: FileState | null): boolean =>
  !!f &&
  f.text !== undefined &&
  (f.kind === "markdown" || f.kind === "svg" || f.kind === "sheet");

/** 两态里「渲染那一态」叫什么、用哪枚图标 —— 按钮显示的是**点了会去哪**，不是当前在哪 */
const RENDER_VIEW: Record<string, { label: string; icon: string }> = {
  markdown: { label: "渲染", icon: "ph:eye" },
  svg: { label: "预览", icon: "ph:eye" },
  sheet: { label: "表格", icon: "ph:table" },
};

interface FilePaneProps {
  taskId: string;
  /** 项目根（即这棵树的根），只用来在头上显示「你在哪台机器的哪个目录里」 */
  cwd?: string;
  onClose: () => void;
}

/**
 * **会话目录的文件查看器**：与正文并排的一列（不是弹层、不是抽屉）。
 *
 * 形制照 VitaAgent 的 `ViewerPane` ＋ `FileViewer`
 * （`web/src/pages/chat/_components/ViewerPane/index.tsx`、
 * `web/src/components/FileViewer/index.tsx`）：一列、可拖宽、只读、
 * 源码态行号 ＋ 逐行高亮 ＋ 折行不横滚，PDF 交给浏览器自带阅读器，
 * 认不出的给「暂无预览」。**这一阶段没有 diff、没有编辑。**
 *
 * **按文件类型渲染**（见 `Kind`）：markdown 默认渲染态、svg 默认预览态、
 * 表格（xlsx/xls/csv/tsv）默认表格态，头部那颗按钮切到源码；其余文本只有源码一态。
 *
 * 两处**必须**与参照不同，都是被这套架构逼的：
 *
 *   1. **取件是慢的。** 参照那边文件就在浏览器手边；我们这边要 hub 点名让那台机器
 *      现读磁盘、结果随**下一轮上报**回来 —— 实测一轮约 31 秒、刚错过一轮 62 秒。
 *      所以这里从第一帧就摆**骨架屏 ＋ 一句话说明为什么要等**，而不是转个圈让人干等；
 *      等过一整轮还没回来就**明说超时并给重试**，不静默失败（见 `POLL_DELAYS`）。
 *   2. **文本/图片由前端自己判。** 客户端只按魔数认图片，文本一律回
 *      `application/octet-stream` —— 拿那个 MIME 判类型的话，所有源码都会被当成
 *      二进制。所以解完 base64 之后**看字节**：有 NUL 或控制字符过多就是二进制。
 *      （这条避免了为看一眼文件去动客户端。）
 *
 * **根是项目根**（`Task.project`），不随终端 `cd` 漂：会话 `cd doc` 之后树也还是整个项目。
 * 读文件同样以项目根为基准（`getTaskFile(…, "project")`），与列目录是同一个根。
 *
 * 路径安全由客户端保证：`rel` 一律相对项目根，客户端 canonicalize 之后必须仍落在
 * 根内，越界直接回「越出会话目录」。前端这边不拼绝对路径、也不接受用户输入路径。
 */
const FilePane: React.FC<FilePaneProps> = ({ taskId, cwd, onClose }) => {
  /** 当前所在的相对目录（"" = 会话根） */
  const [rel, setRel] = useState("");
  const [dirs, setDirs] = useState<string[]>([]);
  const [files, setFiles] = useState<string[]>([]);
  const [dirLoading, setDirLoading] = useState(false);
  /** 这次列目录为什么没成（null = 没失败）。分类见 `Fail` */
  const [dirFail, setDirFail] = useState<Fail | null>(null);

  const [file, setFile] = useState<FileState | null>(null);
  const [fileLoading, setFileLoading] = useState(false);
  /**
   * 当前是不是源码态。**属于「这一个文件」** —— 每次 `openFile` 都归零，
   * 不然在 A.md 里切到源码之后打开 B.svg 会直接落在源码上
   */
  const [source, setSource] = useState(false);
  /** 这次取文件为什么没成（null = 没失败）。分类见 `Fail` */
  const [fileFail, setFileFail] = useState<Fail | null>(null);

  /** 每一次取件的序号：切目录/切文件时旧的那条回来要丢掉，不能覆盖新的 */
  const dirSeq = useRef(0);
  const fileSeq = useRef(0);
  /** 上一个 objectURL，切走时 revoke —— 不放会一直占着内存 */
  const urlRef = useRef("");

  const loadDir = useCallback(
    // `shown`：这一轮已经先摆出了上次的清单（等新清单期间 hub 会一并带回）
    (next: string, attempt = 0, shown = false) => {
      if (!taskId) {
        return;
      }
      const seq = attempt === 0 ? ++dirSeq.current : dirSeq.current;
      if (attempt === 0) {
        setDirFail(null);
        setDirLoading(true);
      }
      // 进目录那一次让设备重新列：不然看到的永远是第一次打开时的样子
      getTaskDirs(taskId, next, attempt === 0)
        .then((res) => {
          if (seq !== dirSeq.current) {
            return;
          }
          if (res.code !== 0) {
            // 服务端明确拒绝（访客访问他人共享设备的文件就是这一支）。
            // **原文照出**：说成「还没送回来」等于告诉人再等等，而它永远不会来
            setDirLoading(false);
            setDirFail({ kind: "deny", msg: res.msg || "读取目录失败" });
            return;
          }
          const stale = res.data?.pending && hasListing(res.data);
          if (stale) {
            // 先摆上次的样子，新的到了再换 —— 回到看过的目录不用干等一轮上报
            setDirs(res.data?.dirs ?? []);
            setFiles(res.data?.files ?? []);
            setDirLoading(false);
          }
          if (res.data?.pending && attempt < POLL_DELAYS.length) {
            window.setTimeout(
              () => loadDir(next, attempt + 1, shown || !!stale),
              POLL_DELAYS[attempt],
            );
            return;
          }
          if (res.data?.pending) {
            // 等不到就明说，别把它渲染成一个空目录；已经摆着上次的清单就留着它
            if (!shown && !stale) {
              setDirFail(timeoutFail("目录"));
            }
            setDirLoading(false);
            return;
          }
          setDirs(res.data?.dirs ?? []);
          setFiles(res.data?.files ?? []);
          setDirLoading(false);
        })
        .catch(() => {
          if (seq === dirSeq.current) {
            setDirLoading(false);
            setDirFail(NETWORK_FAIL);
          }
        });
    },
    [taskId],
  );

  useEffect(() => {
    loadDir("");
    setRel("");
    // 换会话就整个重来
  }, [taskId, loadDir]);

  // 走掉的时候把 objectURL 放掉
  useEffect(
    () => () => {
      if (urlRef.current) {
        URL.revokeObjectURL(urlRef.current);
      }
    },
    [],
  );

  const openDir = (next: string) => {
    setRel(next);
    setDirs([]);
    setFiles([]);
    loadDir(next);
  };

  const openFile = (name: string, attempt = 0) => {
    const full = rel ? `${rel}/${name}` : name;
    const seq = attempt === 0 ? ++fileSeq.current : fileSeq.current;
    if (attempt === 0) {
      setFileFail(null);
      setFileLoading(true);
      setSource(false);
      setFile({ rel: full, name, kind: "text", size: 0 });
    }
    getTaskFile(taskId, full, "project")
      .then((res) => {
        if (seq !== fileSeq.current) {
          return;
        }
        if (res.code !== 0) {
          // 明确的拒绝：访客的 403、安全策略拦下的密钥类文件都走这里。原文照出、不给重试
          setFileLoading(false);
          setFileFail({ kind: "deny", msg: res.msg || "读取失败" });
          return;
        }
        if (res.data?.pending && attempt < POLL_DELAYS.length) {
          window.setTimeout(
            () => openFile(name, attempt + 1),
            POLL_DELAYS[attempt],
          );
          return;
        }
        if (res.data?.pending) {
          setFileLoading(false);
          setFileFail(timeoutFail("文件"));
          return;
        }
        const b64 = res.data?.contentB64 ?? "";
        const bytes = b64ToBytes(b64);
        const ext = extOf(name);
        // 扩展名先说了算，认不出再看字节：`.md` 就是 markdown、`.csv` 就是表，
        // 而没有扩展名的 `Makefile`、`LICENSE` 得靠字节判
        const isText = looksText(bytes);
        let kind: Kind = "binary";
        let text: string | undefined;
        let url: string | undefined;
        let clipped = false;
        if (ext === "pdf") {
          kind = "pdf";
        } else if (ext === "svg") {
          kind = "svg";
        } else if (IMAGE_EXT.includes(ext)) {
          kind = "image";
        } else if (SHEET_EXT.includes(ext)) {
          kind = "sheet";
        } else if (isText) {
          kind = MD_EXT.includes(ext) ? "markdown" : "text";
        }
        // 能给出源码的都把文本解出来：源码态要它，复制也要它。
        // `.xlsx` 这种走不到这儿（`isText` 为假），它那颗切换按钮也就不会出现
        if (isText && kind !== "image" && kind !== "pdf" && kind !== "binary") {
          const whole = new TextDecoder("utf-8").decode(bytes);
          const lines = whole.split("\n");
          clipped = lines.length > MAX_LINES;
          text = clipped ? lines.slice(0, MAX_LINES).join("\n") : whole;
        }
        if (kind === "image" || kind === "pdf" || kind === "svg") {
          const mime =
            kind === "pdf"
              ? "application/pdf"
              : kind === "svg"
                ? "image/svg+xml"
                : `image/${ext === "jpg" ? "jpeg" : ext}`;
          const blob = new Blob([bytes], { type: mime });
          url = URL.createObjectURL(blob);
        }
        if (urlRef.current) {
          URL.revokeObjectURL(urlRef.current);
        }
        urlRef.current = url ?? "";
        setFile({
          rel: full,
          name,
          kind,
          text,
          url,
          size: bytes.length,
          clipped,
          // 表格态要原始字节喂解析器；别的类型留着只是白占内存
          bytes: kind === "sheet" ? bytes : undefined,
        });
        setFileLoading(false);
      })
      .catch(() => {
        if (seq === fileSeq.current) {
          setFileLoading(false);
          setFileFail(NETWORK_FAIL);
        }
      });
  };

  const backToTree = () => {
    fileSeq.current += 1;
    if (urlRef.current) {
      URL.revokeObjectURL(urlRef.current);
      urlRef.current = "";
    }
    setFile(null);
    setFileFail(null);
    setFileLoading(false);
  };

  const upOneLevel = () => {
    const cut = rel.lastIndexOf("/");
    openDir(cut > 0 ? rel.slice(0, cut) : "");
  };

  /** 慢往返那句话。**从第一帧就说**，不是超时之后才说 */
  const waitingHint = (
    <div className={styles.hint}>
      <Icon icon="ph:hourglass-medium" className={styles.hintIcon} />
      <span>
        正在向那台机器要 —— 它要等下一轮上报才会把内容送回来，最长约一分钟
      </span>
    </div>
  );

  const skeleton = (
    <div className={styles.skeleton} aria-hidden>
      {Array.from({ length: 8 }).map((_, i) => (
        <span key={i} className={styles.skelLine} />
      ))}
    </div>
  );

  const renderTree = () => {
    if (dirLoading) {
      return (
        <>
          {waitingHint}
          {skeleton}
        </>
      );
    }
    if (dirFail) {
      return (
        <div className={styles.empty}>
          <div>{dirFail.msg}</div>
          {/* 明确的拒绝不给重试：再点一次还是同一句话 */}
          {dirFail.kind === "deny" ? null : (
            <button
              type="button"
              className={styles.retry}
              onClick={() => loadDir(rel)}
            >
              重试
            </button>
          )}
        </div>
      );
    }
    if (!dirs.length && !files.length) {
      return <div className={styles.empty}>这个目录是空的</div>;
    }
    return (
      <ul className={styles.list}>
        {dirs.map((d) => (
          <li key={`d:${d}`}>
            <button
              type="button"
              className={styles.row}
              onClick={() => openDir(rel ? `${rel}/${d}` : d)}
            >
              <Icon icon="ph:folder" className={styles.rowIcon} />
              <span className={styles.rowName}>{d}</span>
              <Icon icon="ph:caret-right" className={styles.rowGo} />
            </button>
          </li>
        ))}
        {files.map((f) => (
          <li key={`f:${f}`}>
            <button
              type="button"
              className={styles.row}
              onClick={() => openFile(f)}
            >
              <Icon icon="ph:file-text" className={styles.rowIcon} />
              <span className={styles.rowName}>{f}</span>
            </button>
          </li>
        ))}
      </ul>
    );
  };

  const renderFile = () => {
    if (fileLoading) {
      return (
        <>
          {waitingHint}
          {skeleton}
        </>
      );
    }
    if (fileFail) {
      return (
        <div className={styles.empty}>
          <div>{fileFail.msg}</div>
          {/* 同上：被安全策略拒掉的 `.env`、访客的 403，重试一万次结果一样 */}
          {fileFail.kind === "deny" ? null : (
            <button
              type="button"
              className={styles.retry}
              onClick={() => file && openFile(file.name)}
            >
              重试
            </button>
          )}
        </div>
      );
    }
    if (!file) {
      return null;
    }
    if (file.kind === "pdf") {
      /* PDF 交给浏览器自带的阅读器，不引 pdf.js（Chrome / Safari / Edge 都自带，
         多背一个几百 KB 的解析器只为显示一份只读文档不值当）——照参照 */
      return <iframe className={styles.pdf} src={file.url} title={file.name} />;
    }
    if (file.kind === "binary") {
      return (
        <div className={styles.empty}>
          <div>这是二进制文件，没法直接看</div>
          <div className={styles.dim}>
            {extOf(file.name).toUpperCase() || "文件"} ·{" "}
            {Math.max(1, Math.round(file.size / 1024))} KB
          </div>
        </div>
      );
    }
    // 位图，以及**源码态没开着的** svg：都走 `<img>`。
    // svg 走 `<img>` 不是图省事 —— 浏览器对 `<img>` 里的 SVG 用的是「安全静态模式」，
    // 里头的 `<script>` / 事件属性 / 外链一律不执行不加载。这份 svg 来自别人的机器，
    // 内联脚本是要防的，所以**不能**改成把它内联进 DOM
    if (file.kind === "image" || (file.kind === "svg" && !source)) {
      return (
        <div className={styles.media}>
          <img src={file.url} alt={file.name} />
        </div>
      );
    }
    if (file.kind === "sheet" && !source) {
      return file.bytes ? (
        <SheetView bytes={file.bytes} textual={file.text !== undefined} />
      ) : null;
    }
    if (file.kind === "markdown" && !source) {
      /* 渲染走组件库的 `Markdown.Views` —— 与会话正文（`SessionMarkdown`）同一条路径，
         排版也套同一份基线（`styles/_chatMarkdown.scss`）。它不带 `rehype-raw`，
         **markdown 里的裸 HTML 不会被当标签渲染**，正合这份「别人机器上的文件」的定位 */
      return (
        <div className={styles.md}>
          <Markdown.Views>{file.text ?? ""}</Markdown.Views>
        </div>
      );
    }
    return (
      <>
        {file.clipped ? (
          <div className={styles.clip}>
            文件太长，只显示前 {MAX_LINES} 行
          </div>
        ) : null}
        <CodeLines text={file.text ?? ""} lang={extOf(file.name)} />
      </>
    );
  };

  /** 此刻正在画表格（不是骨架屏、不是失败态、也不是切到了源码） */
  const sheetView =
    !fileLoading && !fileFail && file?.kind === "sheet" && !source;

  const copyText = () => {
    if (!file?.text) {
      return;
    }
    navigator.clipboard
      ?.writeText(file.text)
      .then(() => message.success("已复制"))
      .catch(() => message.error("复制失败"));
  };

  return (
    <div className={styles.FilePane}>
      <div className={styles.head}>
        {file ? (
          <Tooltip title="回到文件列表">
            <button type="button" className={styles.headBtn} onClick={backToTree}>
              <Icon icon="ph:caret-left" />
            </button>
          </Tooltip>
        ) : null}
        <span className={styles.headPath} title={cwd ? `${cwd}/${rel}` : rel}>
          {file ? file.name : rel || "会话目录"}
        </span>
        {/* 两态切换：与复制、关闭同一排、同一规格（`.headBtn`）。
            只在真有两态的类型下出现 —— `.xlsx` 给不出源码，就没有这颗按钮 */}
        {hasTwoViews(file) && file ? (
          <Tooltip title={source ? RENDER_VIEW[file.kind].label : "源码"}>
            <button
              type="button"
              className={styles.headBtn}
              onClick={() => setSource((v) => !v)}
              aria-label={source ? RENDER_VIEW[file.kind].label : "源码"}
            >
              <Icon icon={source ? RENDER_VIEW[file.kind].icon : "ph:code"} />
            </button>
          </Tooltip>
        ) : null}
        {/* 有源码就能复制：markdown / svg / csv 在渲染态下复制的也是这份源码 */}
        {file?.text !== undefined ? (
          <Tooltip title="复制全文">
            <button type="button" className={styles.headBtn} onClick={copyText}>
              <Icon icon="ph:copy" />
            </button>
          </Tooltip>
        ) : null}
        <Tooltip title="关闭文件查看">
          <button
            type="button"
            className={styles.headBtn}
            onClick={onClose}
            aria-label="关闭文件查看"
          >
            <Icon icon="ph:x" />
          </button>
        </Tooltip>
      </div>

      {/* 面包屑只在进了子目录、且没在看文件时给：根目录不需要一行「返回上级」 */}
      {!file && rel ? (
        <button type="button" className={styles.up} onClick={upOneLevel}>
          <Icon icon="ph:arrow-u-left-up" className={styles.upIcon} />
          <span>上级目录</span>
        </button>
      ) : null}

      {/* 表格态要把高度**钉死**交给表自己滚（组件库的 Table 是 height:100% 的 flex 列，
          父级没有确定高度就画不出表体）。其余形态仍是「内容多高就多高、这一层滚」 */}
      <div
        className={classNames(styles.body, { [styles.bodyFill]: sheetView })}
      >
        {file ? renderFile() : renderTree()}
      </div>
    </div>
  );
};

export default FilePane;
