import React, { useEffect, useRef, useState } from "react";

import { Chat, Input, Modal } from "@hsu-react/ui";
import { message } from "antd";
import { reaction } from "mobx";
import {
  DeleteOutlined,
  EditOutlined,
  FileSearchOutlined,
  FolderAddOutlined,
  PaperClipOutlined,
  WarningOutlined,
} from "@ant-design/icons";

import {
  SlashCommand,
  fsopTask,
  getFsopResult,
  getPortalSlashCommands,
  getTaskDirs,
  uploadPortalFile,
} from "@/services/apis/portal";
import { CONFIRM_WORD, DangerHit, checkDanger } from "../../_utils/dangerCheck";
import PortalStore from "../../PortalStore";
import styles from "./index.module.scss";

interface ComposerProps {
  taskId: string;
  disabled?: boolean;
  onSend: (text: string) => void;
  /** 会话所在设备（上传文件的目标） */
  machineId?: string;
  /** 会话工作目录（上传落点；回填的相对路径以此为基准） */
  cwd?: string;
}

/**
 * Claude 式对话输入框（hsu-ui Chat.Input）：
 * - 上方为该模型可用斜杠命令 chips，点击直接发布；
 * - 命中危险模式（类 Claude Code bypass 权限等）时走两步确认。
 */
const Composer: React.FC<ComposerProps> = (props) => {
  const { taskId, disabled, onSend, machineId, cwd } = props;
  const [commands, setCommands] = useState<SlashCommand[]>([]);
  const [uploading, setUploading] = useState(false);
  // 斜杠命令：仅当输入以「/」开头且未含空格时弹出（Claude Code 终端式），
  // null=不在命令模式，字符串=「/」之后已输入的过滤词
  const [slashQuery, setSlashQuery] = useState<string | null>(null);
  // 命令菜单里方向键高亮的项索引
  const [slashActive, setSlashActive] = useState(0);
  const rootRef = useRef<HTMLDivElement>(null);
  const fileRef = useRef<HTMLInputElement>(null);
  // 供原生 keydown 捕获处理器读取当前菜单项/高亮（避免闭包拿到旧值）
  const slashItemsRef = useRef<Array<{ key: string; run: () => void }>>([]);
  const slashActiveRef = useRef(0);

  /**
   * 把文本追加进 Chat.Input 的输入框。
   * Chat.Input 没有受控 value，这里用原生 setter + input 事件驱动其内部
   * 受控 textarea 更新（React 对 textarea 的标准程序化注入方式）。
   */
  const appendToInput = (text: string) => {
    const ta = rootRef.current?.querySelector("textarea");
    if (!ta) return;
    const setter = Object.getOwnPropertyDescriptor(
      window.HTMLTextAreaElement.prototype,
      "value",
    )?.set;
    if (!setter) return;
    const next = ta.value ? `${ta.value} ${text}` : text;
    setter.call(ta, next);
    ta.dispatchEvent(new Event("input", { bubbles: true }));
    ta.focus();
  };

  // 撤回后把原文回填进本会话的对话框（PortalStore.composerRefill 命中自己的 taskId
  // 才消费），方便改完再发。Composer 非 observer，用 reaction 订阅这一个字段即可。
  useEffect(() => {
    const dispose = reaction(
      () => PortalStore.composerRefill,
      (r) => {
        if (r && r.taskId === taskId) {
          appendToInput(r.text);
          PortalStore.consumeComposerRefill();
        }
      },
    );
    return dispose;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [taskId]);

  // 监听输入框内容：以「/xxx」（无空格）开头就进命令模式并按 xxx 过滤。
  // 关键：setSlashQuery 必须延后一帧（rAF）再调用 —— 直接在原生 input 事件里 setState 会
  // 触发重渲染，把 hsu-ui 受控 textarea 的值回滚成本次按键前的旧值（表现为「/ 和字母都要
  // 按两次、连打两个不同字母只留第二个、输入框里根本不显示」）。延后到下一帧时，hsu-ui 的
  // onChange 已把值落定，此时更新 slashQuery 不会再回滚当前按键。
  useEffect(() => {
    const ta = rootRef.current?.querySelector("textarea");
    if (!ta) return;
    let raf = 0;
    const onInput = () => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => {
        const v = ta.value;
        const m = /^\/(\S*)$/.exec(v);
        setSlashQuery(m ? m[1] : null);
      });
    };
    const onBlur = () => setTimeout(() => setSlashQuery(null), 150);
    ta.addEventListener("input", onInput);
    ta.addEventListener("blur", onBlur);
    return () => {
      cancelAnimationFrame(raf);
      ta.removeEventListener("input", onInput);
      ta.removeEventListener("blur", onBlur);
    };
  }, [taskId]);

  // 移动端：回车换行、不发送（发送用右下角发送按钮）。hsu-ui Chat.Input 默认回车即提交，
  // 这里在捕获阶段拦住移动端的 Enter、stopPropagation 阻止它到达 Chat.Input 的提交处理，
  // 不 preventDefault 让 textarea 自然插入换行。
  useEffect(() => {
    const ta = rootRef.current?.querySelector("textarea");
    if (!ta) return;
    const onKeyDownCapture = (e: KeyboardEvent) => {
      // 命令菜单开着时：↑↓ 移高亮、回车选中当前项（选中后自动聚焦回输入框）
      const items = slashItemsRef.current;
      if (items.length > 0 && !e.isComposing) {
        if (e.key === "ArrowDown") {
          e.preventDefault();
          e.stopPropagation();
          setSlashActive((i) => (i + 1) % items.length);
          return;
        }
        if (e.key === "ArrowUp") {
          e.preventDefault();
          e.stopPropagation();
          setSlashActive((i) => (i - 1 + items.length) % items.length);
          return;
        }
        if (e.key === "Enter" && !e.shiftKey) {
          e.preventDefault();
          e.stopPropagation();
          items[slashActiveRef.current]?.run();
          rootRef.current?.querySelector("textarea")?.focus();
          return;
        }
      }
      const isMobile = window.matchMedia("(max-width: 760px)").matches;
      if (isMobile && e.key === "Enter" && !e.shiftKey && !e.isComposing) {
        e.stopPropagation();
      }
    };
    ta.addEventListener("keydown", onKeyDownCapture, true);
    return () => ta.removeEventListener("keydown", onKeyDownCapture, true);
  }, [taskId]);

  // 把某条命令填进输入框（保留在输入框，用户可继续补参数或直接回车发布）
  const fillCommand = (name: string) => {
    const ta = rootRef.current?.querySelector("textarea");
    if (!ta) return;
    const setter = Object.getOwnPropertyDescriptor(
      window.HTMLTextAreaElement.prototype,
      "value",
    )?.set;
    setter?.call(ta, `${name} `);
    ta.dispatchEvent(new Event("input", { bubbles: true }));
    ta.focus();
    setSlashQuery(null);
  };

  // 选中的待上传文件 + 目录树浏览（根 = 会话所在目录，只能往下走）
  const [pendingFile, setPendingFile] = useState<File | null>(null);
  const [dirRel, setDirRel] = useState("");
  const [dirList, setDirList] = useState<string[]>([]);
  const [dirFiles, setDirFiles] = useState<string[]>([]);
  const [dirLoading, setDirLoading] = useState(false);
  const dirPollRef = useRef(0);
  // 「选择文件回填相对路径」模态（与上传共用目录浏览，但只读、点文件即插入路径）
  const [pickerOpen, setPickerOpen] = useState(false);

  // 拉取 rel 下的子目录；agent 异步回带，pending 时 1.2s 后重试（最多 8 次）
  const loadDirs = (rel: string, attempt = 0) => {
    if (!taskId) return;
    const seq = ++dirPollRef.current;
    setDirLoading(true);
    getTaskDirs(taskId, rel)
      .then((res) => {
        if (seq !== dirPollRef.current) return;
        if (res.code !== 0) {
          setDirLoading(false);
          message.error(res.msg ?? "读取目录失败");
          return;
        }
        if (res.data?.pending && attempt < 8) {
          window.setTimeout(() => loadDirs(rel, attempt + 1), 1200);
          return;
        }
        setDirList(res.data?.dirs ?? []);
        setDirFiles(res.data?.files ?? []);
        setDirLoading(false);
      })
      .catch(() => {
        if (seq === dirPollRef.current) setDirLoading(false);
      });
  };

  const enterDir = (name: string) => {
    const next = dirRel ? `${dirRel}/${name}` : name;
    setDirRel(next);
    setDirList([]);
    loadDirs(next);
  };

  const upDir = () => {
    const next = dirRel.split("/").slice(0, -1).join("/");
    setDirRel(next);
    setDirList([]);
    loadDirs(next);
  };

  // 文件夹操作忙标记（防连点）
  const [fsBusy, setFsBusy] = useState(false);

  /** 执行文件夹操作：下发 → 轮询结果 → 提示 → 重拉当前目录 */
  const runFsop = async (
    op: "mkdir" | "delete" | "rename",
    name: string,
    newName?: string,
  ) => {
    if (!taskId || fsBusy) return;
    setFsBusy(true);
    const hide = message.loading(
      op === "mkdir" ? "新建中…" : op === "delete" ? "删除中…" : "重命名中…",
      0,
    );
    try {
      const res = await fsopTask(taskId, { op, rel: dirRel, name, newName });
      if (res.code !== 0 || !res.data?.opId) {
        message.error(res.msg ?? "操作失败");
        return;
      }
      const opId = res.data.opId;
      // agent 下一轮上报（≤1.5s）才执行，轮询取结果（最多 ~12s）
      let done = false;
      for (let i = 0; i < 12 && !done; i++) {
        await new Promise((r) => window.setTimeout(r, 1000));
        const rr = await getFsopResult(taskId, opId);
        if (rr.code === 0 && rr.data && !rr.data.pending) {
          done = true;
          if (rr.data.ok) message.success(rr.data.msg || "已完成");
          else message.error(rr.data.msg || "操作失败");
        }
      }
      if (!done) message.warning("操作已下发，稍后刷新目录查看");
    } catch {
      message.error("操作失败，请检查网络");
    } finally {
      hide();
      setFsBusy(false);
      loadDirs(dirRel); // 无论成败都重拉，反映最新目录
    }
  };

  const newFolder = () => {
    const name = window.prompt("新建文件夹名称")?.trim();
    if (!name) return;
    if (/[\\/]/.test(name)) {
      message.warning("名称不能包含斜杠");
      return;
    }
    runFsop("mkdir", name);
  };

  const renameFolder = (name: string) => {
    const next = window.prompt("重命名文件夹", name)?.trim();
    if (!next || next === name) return;
    if (/[\\/]/.test(next)) {
      message.warning("名称不能包含斜杠");
      return;
    }
    runFsop("rename", name, next);
  };

  const deleteFolder = (name: string) => {
    if (!window.confirm(`删除文件夹「${name}」及其全部内容？此操作不可恢复。`)) return;
    runFsop("delete", name);
  };

  const onPickFile = (file: File) => {
    if (!machineId || !cwd) {
      message.warning("该会话缺少设备或目录信息，无法传文件");
      return;
    }
    setPendingFile(file);
    setDirRel("");
    setDirList([]);
    setDirFiles([]);
    loadDirs("");
  };

  // 打开「选择文件」浏览器：根 = 会话所在目录
  const openPicker = () => {
    setPickerOpen(true);
    setDirRel("");
    setDirList([]);
    setDirFiles([]);
    loadDirs("");
  };

  // 选中某个文件 → 把相对会话目录的路径（正斜杠通用）插入输入框
  const pickFileRef = (name: string) => {
    appendToInput(`./${dirRel ? `${dirRel}/` : ""}${name}`);
    setPickerOpen(false);
  };

  // 拖拽中高亮
  const [dragOver, setDragOver] = useState(false);

  // 粘贴图片/文件 → 直接进上传流程（图片粘贴常无文件名，补个默认名）
  const onPaste = (e: React.ClipboardEvent) => {
    if (disabled) return;
    const items = Array.from(e.clipboardData?.items ?? []);
    const fileItem = items.find((it) => it.kind === "file");
    if (!fileItem) return;
    const f = fileItem.getAsFile();
    if (!f) return;
    e.preventDefault();
    const named =
      f.name && f.name !== "image.png"
        ? f
        : new File([f], `粘贴-${Date.now()}.${(f.type.split("/")[1] || "png")}`, {
            type: f.type,
          });
    onPickFile(named);
  };

  /** 确认上传到当前浏览目录，成功后把相对路径填入输入框 */
  const doUpload = () => {
    const file = pendingFile;
    if (!file || !machineId || !cwd) {
      return;
    }
    // 设备侧绝对目录 = 会话目录 + 相对子路径（按设备的分隔符拼）
    const sep = cwd.includes("\\") ? "\\" : "/";
    const dir = dirRel ? `${cwd}${sep}${dirRel.split("/").join(sep)}` : cwd;
    setUploading(true);
    setPendingFile(null);
    uploadPortalFile(machineId, dir, file)
      .then((res) => {
        if (res.code === 0) {
          message.success(res.data?.result ?? "已上传");
          // 回填相对路径（相对会话目录，正斜杠通用）
          appendToInput(dirRel ? `./${dirRel}/${file.name}` : `./${file.name}`);
        } else {
          message.error(res.msg ?? "上传失败");
        }
      })
      .catch(() => message.error("上传失败，请检查网络"))
      .finally(() => setUploading(false));
  };

  // 会话切换时拉取该模型的可用命令（只读、不影响任务）
  useEffect(() => {
    if (!taskId) return;
    // 竞态门闩：快速切换会话时，先发的请求可能后返回，
    // 不拦截的话旧会话的命令列表会盖掉新会话的。
    let alive = true;
    getPortalSlashCommands(taskId)
      .then((res) => {
        if (alive && res.code === 0) setCommands(res.data?.list ?? []);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [taskId]);

  // 危险输入多重确认：第 1 步警告说明，第 2 步输入确认词
  const [dangerHits, setDangerHits] = useState<DangerHit[]>([]);
  const [dangerText, setDangerText] = useState("");
  const [dangerStep, setDangerStep] = useState<0 | 1 | 2>(0);
  const [confirmInput, setConfirmInput] = useState("");

  const closeDanger = () => {
    setDangerStep(0);
    setDangerHits([]);
    setDangerText("");
    setConfirmInput("");
  };

  const guardedSend = (raw: string) => {
    const text = raw.trim();
    if (!text) return;
    if (disabled) {
      message.warning("该会话无存活进程，无法发布");
      return;
    }

    const hits = checkDanger(text);
    if (hits.length > 0) {
      setDangerHits(hits);
      setDangerText(text);
      setDangerStep(1);
      return;
    }
    onSend(text);
  };

  const confirmDanger = () => {
    if (dangerStep === 1) {
      setDangerStep(2);
      return;
    }
    if (confirmInput.trim() === CONFIRM_WORD) {
      onSend(dangerText);
      closeDanger();
    }
  };

  // 命令模式下按「/」后的过滤词匹配（前缀 + 描述），最多 8 条
  const matched =
    slashQuery === null
      ? []
      : commands
          .filter((c) => {
            const q = slashQuery.toLowerCase();
            return (
              c.name.toLowerCase().includes("/" + q) ||
              c.name.toLowerCase().slice(1).startsWith(q)
            );
          })
          .slice(0, 8);

  // 允许输入不在列表里的自定义「/命令」：只要不是恰好命中某条命令，就在列表顶部给一个
  // 「直接发送」项，点它把当前输入原样发出（列表只是建议、从不拦截自定义命令）。
  const exactMatch =
    slashQuery !== null &&
    commands.some(
      (c) => c.name.toLowerCase() === `/${slashQuery.toLowerCase()}`,
    );
  const showCustom = slashQuery !== null && slashQuery !== "" && !exactMatch;

  const sendCustom = () => {
    const ta = rootRef.current?.querySelector("textarea");
    const v = ta?.value ?? "";
    if (!v.trim()) return;
    guardedSend(v);
    if (ta) {
      const setter = Object.getOwnPropertyDescriptor(
        window.HTMLTextAreaElement.prototype,
        "value",
      )?.set;
      setter?.call(ta, "");
      ta.dispatchEvent(new Event("input", { bubbles: true }));
      ta.focus();
    }
    setSlashQuery(null);
  };

  // 命令菜单项（含顶部「发送自定义」项）：方向键/回车导航与渲染共用同一份顺序。
  // 期间用 ref 暴露给原生 keydown 处理器，避免闭包读到旧值。
  const slashItems: Array<{ key: string; run: () => void }> = [];
  if (showCustom) slashItems.push({ key: "__custom", run: sendCustom });
  matched.forEach((c) => slashItems.push({ key: c.name, run: () => fillCommand(c.name) }));
  slashItemsRef.current = slashItems;
  slashActiveRef.current = Math.min(slashActive, Math.max(0, slashItems.length - 1));

  // 菜单重开 / 换过滤词 / 集合变化时，高亮回到第一项
  useEffect(() => {
    setSlashActive(0);
  }, [slashQuery]);

  return (
    <div
      className={`${styles.Composer} ${dragOver ? styles.dragOver : ""}`}
      ref={rootRef}
      onPaste={onPaste}
      onDragOver={(e) => {
        if (disabled) return;
        if (Array.from(e.dataTransfer?.types ?? []).includes("Files")) {
          e.preventDefault();
          setDragOver(true);
        }
      }}
      onDragLeave={(e) => {
        // 只在真正离开根容器时取消（子元素间移动不算）
        if (e.currentTarget === e.target) setDragOver(false);
      }}
      onDrop={(e) => {
        e.preventDefault();
        setDragOver(false);
        if (disabled) return;
        const f = e.dataTransfer?.files?.[0];
        if (f) onPickFile(f);
      }}
    >
      {dragOver ? (
        <div className={styles.dropHint}>松开上传文件到会话目录</div>
      ) : null}
      {/* 斜杠命令下拉：仅在输入「/」时弹出（Claude Code 终端式），
          不再常驻一排命令 chip */}
      {(matched.length > 0 || showCustom) && !disabled && (
        <div className={styles.slashMenu}>
          {showCustom ? (
            <div
              className={`${styles.slashItem} ${styles.slashCustom} ${
                slashActive === 0 ? styles.slashActiveItem : ""
              }`}
              role="button"
              tabIndex={0}
              onMouseEnter={() => setSlashActive(0)}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => {
                sendCustom();
                rootRef.current?.querySelector("textarea")?.focus();
              }}
            >
              <span className={styles.slashName}>发送 /{slashQuery}</span>
              <span className={styles.slashDesc}>不在列表中的自定义命令，直接发出</span>
            </div>
          ) : null}
          {matched.map((c, i) => {
            const idx = showCustom ? i + 1 : i;
            return (
              <div
                key={c.name}
                className={`${styles.slashItem} ${
                  slashActive === idx ? styles.slashActiveItem : ""
                }`}
                role="button"
                tabIndex={0}
                onMouseEnter={() => setSlashActive(idx)}
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => {
                  fillCommand(c.name);
                  rootRef.current?.querySelector("textarea")?.focus();
                }}
              >
                <span className={styles.slashName}>{c.name}</span>
                {c.desc ? <span className={styles.slashDesc}>{c.desc}</span> : null}
              </div>
            );
          })}
        </div>
      )}

      {/* 注意：不传 assistanting —— 本产品要向「执行中」的会话注入输入 */}
      <Chat.Input
        wrapperClassName={styles.chatInput}
        placeholder={
          disabled ? "该会话无存活进程，无法发布" : "输入任务，回车发布"
        }
        onSend={guardedSend}
        uploadEnabled={false}
        buttonGroup={
          cwd && !disabled
            ? [
                {
                  title: "选择会话目录里的文件，插入相对路径",
                  // FileSearchOutlined 字形本身偏小，略调大与旁边回形针视觉一致
                  icon: (
                    <FileSearchOutlined
                      className={styles.uploadIcon}
                      style={{ fontSize: 18 }}
                    />
                  ),
                  type: "text",
                  onClick: openPicker,
                },
                ...(machineId
                  ? [
                      {
                        title: "传文件到会话目录（完成后自动填入路径）",
                        icon: <PaperClipOutlined className={styles.uploadIcon} />,
                        type: "text" as const,
                        loading: uploading,
                        onClick: () => fileRef.current?.click(),
                      },
                    ]
                  : []),
              ]
            : undefined
        }
      />
      {/* 上传目录确认：默认会话所在目录，可改成设备上任意目录 */}
      <Modal
        title="传文件到设备"
        open={!!pendingFile}
        onCancel={() => setPendingFile(null)}
        onOk={doUpload}
        okText="上传"
        cancelText="取消"
        width={460}
        centered
      >
        <div className={styles.uploadForm}>
          <div className={styles.uploadFile}>
            文件：<b>{pendingFile?.name}</b>
          </div>
          <div className={styles.uploadLabel}>
            <span>目标目录（会话目录内选择）</span>
            <span
              className={styles.dirNewBtn}
              role="button"
              tabIndex={0}
              onClick={newFolder}
            >
              <FolderAddOutlined /> 新建文件夹
            </span>
          </div>
          <div className={styles.dirCrumb}>
            会话目录{dirRel ? ` / ${dirRel.split("/").join(" / ")}` : ""}
          </div>
          <div className={styles.dirTree}>
            {dirRel ? (
              <div className={styles.dirItem} onClick={upDir} role="button" tabIndex={0}>
                <span className={styles.dirIcon}>↩</span> 返回上级
              </div>
            ) : null}
            {dirLoading ? (
              <div className={styles.dirEmpty}>读取目录中…</div>
            ) : dirList.length === 0 ? (
              <div className={styles.dirEmpty}>{dirRel ? "没有子目录" : "该目录下没有子目录"}</div>
            ) : (
              dirList.map((d) => (
                <div key={d} className={styles.dirItem}>
                  <span
                    className={styles.dirItemName}
                    role="button"
                    tabIndex={0}
                    onClick={() => enterDir(d)}
                  >
                    <span className={styles.dirIcon}>📁</span> {d}
                  </span>
                  <span className={styles.dirItemOps}>
                    <EditOutlined
                      title="重命名"
                      onClick={(e) => {
                        e.stopPropagation();
                        renameFolder(d);
                      }}
                    />
                    <DeleteOutlined
                      title="删除"
                      onClick={(e) => {
                        e.stopPropagation();
                        deleteFolder(d);
                      }}
                    />
                  </span>
                </div>
              ))
            )}
          </div>
          <div className={styles.uploadHint}>
            将上传到：<b>./{dirRel ? `${dirRel}/` : ""}{pendingFile?.name}</b>
          </div>
        </div>
      </Modal>

      {/* 选择文件：浏览会话目录，点文件即把相对路径插入输入框（不上传） */}
      <Modal
        title="选择文件（插入相对路径）"
        open={pickerOpen}
        onCancel={() => setPickerOpen(false)}
        footer={null}
        width={460}
        centered
      >
        <div className={styles.uploadForm}>
          <div className={styles.dirCrumb}>
            会话目录{dirRel ? ` / ${dirRel.split("/").join(" / ")}` : ""}
          </div>
          <div className={styles.dirTree}>
            {dirRel ? (
              <div className={styles.dirItem} onClick={upDir} role="button" tabIndex={0}>
                <span className={styles.dirIcon}>↩</span> 返回上级
              </div>
            ) : null}
            {dirLoading ? (
              <div className={styles.dirEmpty}>读取目录中…</div>
            ) : dirList.length === 0 && dirFiles.length === 0 ? (
              <div className={styles.dirEmpty}>该目录为空</div>
            ) : (
              <>
                {dirList.map((d) => (
                  <div
                    key={`d-${d}`}
                    className={styles.dirItem}
                    role="button"
                    tabIndex={0}
                    onClick={() => enterDir(d)}
                  >
                    <span className={styles.dirIcon}>📁</span> {d}
                  </div>
                ))}
                {dirFiles.map((f) => (
                  <div
                    key={`f-${f}`}
                    className={styles.dirItem}
                    role="button"
                    tabIndex={0}
                    onClick={() => pickFileRef(f)}
                  >
                    <span className={styles.dirIcon}>📄</span> {f}
                  </div>
                ))}
              </>
            )}
          </div>
          <div className={styles.uploadHint}>
            点击文件即插入：<b>./{dirRel ? `${dirRel}/` : ""}文件名</b>
          </div>
        </div>
      </Modal>

      {/* 隐藏的文件选择器（由工具栏回形针按钮触发） */}
      <input
        ref={fileRef}
        type="file"
        hidden
        onChange={(e) => {
          const f = e.target.files?.[0];
          if (f) onPickFile(f);
          // 允许连续选同一个文件
          e.target.value = "";
        }}
      />

      {/* 危险输入多重确认（类 Claude Code bypass 权限等需特别管理） */}
      <Modal
        className={styles.dangerModal}
        title={
          <span className={styles.dangerTitle}>
            <WarningOutlined /> 高危操作确认（{dangerStep}/2）
          </span>
        }
        open={dangerStep > 0}
        onCancel={closeDanger}
        onOk={confirmDanger}
        okText={dangerStep === 1 ? "我已知晓风险，继续" : "确认执行"}
        cancelText="取消"
        okButtonProps={{
          danger: true,
          disabled: dangerStep === 2 && confirmInput.trim() !== CONFIRM_WORD,
        }}
        width={480}
      >
        <div className={styles.dangerBody}>
          <div className={styles.dangerText}>
            即将发布的内容命中以下高危模式，将直接作用于目标机器的终端进程：
          </div>
          <ul className={styles.dangerList}>
            {dangerHits.map((h) => (
              <li key={h.pattern}>
                <code>{h.pattern}</code>
                <span>{h.desc}</span>
              </li>
            ))}
          </ul>
          <div className={styles.dangerDraft}>{dangerText}</div>
          {dangerStep === 2 && (
            <div className={styles.dangerConfirm}>
              <div className={styles.dangerText}>
                请输入 <strong>{CONFIRM_WORD}</strong> 以最终确认：
              </div>
              <Input
                value={confirmInput}
                onChange={(value) => setConfirmInput(value)}
                placeholder={CONFIRM_WORD}
              />
            </div>
          )}
        </div>
      </Modal>
    </div>
  );
};

export default Composer;
