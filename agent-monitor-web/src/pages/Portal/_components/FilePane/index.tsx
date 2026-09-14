import React, { useCallback, useEffect, useRef, useState } from "react";

import { Icon } from "@hsu-react/ui";
import { Tooltip, message } from "antd";

import { getTaskDirs, getTaskFile } from "@/services/apis/portal";
import CodeLines from "./CodeLines";
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

const IMAGE_EXT = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico"];

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

type Kind = "text" | "image" | "pdf" | "binary";

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
}

interface FilePaneProps {
  taskId: string;
  /** 会话锚定目录，只用来在头上显示「你在哪台机器的哪个目录里」 */
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
 * 路径安全由客户端保证：`rel` 一律相对会话根，客户端 canonicalize 之后必须仍落在
 * `live_cwd` 内，越界直接回「越出会话目录」。前端这边不拼绝对路径、也不接受用户输入路径。
 */
const FilePane: React.FC<FilePaneProps> = ({ taskId, cwd, onClose }) => {
  /** 当前所在的相对目录（"" = 会话根） */
  const [rel, setRel] = useState("");
  const [dirs, setDirs] = useState<string[]>([]);
  const [files, setFiles] = useState<string[]>([]);
  const [dirLoading, setDirLoading] = useState(false);
  const [dirTimedOut, setDirTimedOut] = useState(false);

  const [file, setFile] = useState<FileState | null>(null);
  const [fileLoading, setFileLoading] = useState(false);
  const [fileErr, setFileErr] = useState("");

  /** 每一次取件的序号：切目录/切文件时旧的那条回来要丢掉，不能覆盖新的 */
  const dirSeq = useRef(0);
  const fileSeq = useRef(0);
  /** 上一个 objectURL，切走时 revoke —— 不放会一直占着内存 */
  const urlRef = useRef("");

  const loadDir = useCallback(
    (next: string, attempt = 0) => {
      if (!taskId) {
        return;
      }
      const seq = attempt === 0 ? ++dirSeq.current : dirSeq.current;
      if (attempt === 0) {
        setDirTimedOut(false);
        setDirLoading(true);
      }
      getTaskDirs(taskId, next)
        .then((res) => {
          if (seq !== dirSeq.current) {
            return;
          }
          if (res.code !== 0) {
            setDirLoading(false);
            setDirTimedOut(true);
            return;
          }
          if (res.data?.pending && attempt < POLL_DELAYS.length) {
            window.setTimeout(
              () => loadDir(next, attempt + 1),
              POLL_DELAYS[attempt],
            );
            return;
          }
          if (res.data?.pending) {
            // 等不到就明说，别把它渲染成一个空目录
            setDirTimedOut(true);
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
            setDirTimedOut(true);
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
      setFileErr("");
      setFileLoading(true);
      setFile({ rel: full, name, kind: "text", size: 0 });
    }
    getTaskFile(taskId, full)
      .then((res) => {
        if (seq !== fileSeq.current) {
          return;
        }
        if (res.code !== 0) {
          setFileLoading(false);
          setFileErr(res.msg || "读取失败");
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
          setFileErr("timeout");
          return;
        }
        const b64 = res.data?.contentB64 ?? "";
        const bytes = b64ToBytes(b64);
        const ext = extOf(name);
        let kind: Kind = "binary";
        let text: string | undefined;
        let url: string | undefined;
        let clipped = false;
        if (ext === "pdf") {
          kind = "pdf";
        } else if (IMAGE_EXT.includes(ext)) {
          kind = "image";
        } else if (looksText(bytes)) {
          kind = "text";
          const whole = new TextDecoder("utf-8").decode(bytes);
          const lines = whole.split("\n");
          clipped = lines.length > MAX_LINES;
          text = clipped ? lines.slice(0, MAX_LINES).join("\n") : whole;
        }
        if (kind === "image" || kind === "pdf") {
          const mime =
            kind === "pdf"
              ? "application/pdf"
              : ext === "svg"
                ? "image/svg+xml"
                : `image/${ext === "jpg" ? "jpeg" : ext}`;
          const blob = new Blob([bytes], { type: mime });
          url = URL.createObjectURL(blob);
        }
        if (urlRef.current) {
          URL.revokeObjectURL(urlRef.current);
        }
        urlRef.current = url ?? "";
        setFile({ rel: full, name, kind, text, url, size: bytes.length, clipped });
        setFileLoading(false);
      })
      .catch(() => {
        if (seq === fileSeq.current) {
          setFileLoading(false);
          setFileErr("读取失败，请检查网络");
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
    setFileErr("");
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
    if (dirTimedOut) {
      return (
        <div className={styles.empty}>
          <div>这台机器还没把目录送回来（一轮上报约 30 秒，最坏 60 秒）</div>
          <button
            type="button"
            className={styles.retry}
            onClick={() => loadDir(rel)}
          >
            重试
          </button>
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
    if (fileErr) {
      return (
        <div className={styles.empty}>
          <div>
            {fileErr === "timeout"
              ? "这台机器还没把文件送回来（一轮上报约 30 秒，最坏 60 秒）"
              : fileErr}
          </div>
          <button
            type="button"
            className={styles.retry}
            onClick={() => file && openFile(file.name)}
          >
            重试
          </button>
        </div>
      );
    }
    if (!file) {
      return null;
    }
    if (file.kind === "image") {
      return (
        <div className={styles.media}>
          <img src={file.url} alt={file.name} />
        </div>
      );
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
        {file?.kind === "text" ? (
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

      <div className={styles.body}>{file ? renderFile() : renderTree()}</div>
    </div>
  );
};

export default FilePane;
