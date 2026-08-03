import React, { useEffect, useRef, useState } from "react";

import { Chat, Input, Modal } from "@hsu-react/ui";
import { message } from "antd";
import { reaction } from "mobx";
import {
  DeleteOutlined,
  EditOutlined,
  FileSearchOutlined,
  FolderAddOutlined,
  HistoryOutlined,
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
import HistoryModal from "../HistoryModal";
import styles from "./index.module.scss";

interface ComposerProps {
  taskId: string;
  disabled?: boolean;
  /**
   * 禁用原因（占位符与拦截提示都用它）。不给就按「没有存活进程」说 ——
   * 但禁用的理由不止这一个（比如已暂停），说错了会把人引到错误的排查方向。
   */
  disabledHint?: string;
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
  const { taskId, disabled, disabledHint, onSend, machineId, cwd } = props;
  const offHint = disabledHint || "该会话无存活进程，无法发布";
  const [commands, setCommands] = useState<SlashCommand[]>([]);
  const [uploading, setUploading] = useState(false);
  // 会话历史弹窗（与当前会话状态无关，任何时候都能翻）
  const [historyOpen, setHistoryOpen] = useState(false);
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
  // 输入法是否正在组字。事件自带的 isComposing 不够：不少输入法（尤其 macOS）
  // 在候选词上屏时是「先 compositionend、再补一个 Enter」的顺序，那个 Enter 上
  // isComposing 已经是 false，看起来就是一次正常的回车。
  const composingRef = useRef(false);

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
  //
  // 同一处还要拦住**输入法上屏用的那个 Enter**：桌面端的提交在 Chat.Input 内部，
  // 组字中的回车一旦漏过去就会把「刚上屏的半句话」当成一条消息发出去。发完输入框
  // 被清空、输入法紧接着又把候选词补回来，于是越打越长、每次上屏都发一条 ——
  // 表现就是同一句话被拆成「卷管理弹窗」「卷管理弹窗表格」这样的递增前缀连发。
  useEffect(() => {
    const ta = rootRef.current?.querySelector("textarea");
    if (!ta) return;
    const onCompStart = () => {
      composingRef.current = true;
    };
    // 延后一个宏任务再解除：紧跟在 compositionend 之后补发的那个 Enter 仍要算组字期内
    const onCompEnd = () => {
      setTimeout(() => {
        composingRef.current = false;
      }, 0);
    };
    const onKeyDownCapture = (e: KeyboardEvent) => {
      // keyCode 229 = 按键被输入法吃掉了，同样不能当回车用
      const composing = composingRef.current || e.isComposing || e.keyCode === 229;
      if (e.key === "Enter" && !e.shiftKey && composing) {
        // 只拦提交，不 preventDefault —— 上屏动作要照常完成
        e.stopPropagation();
        return;
      }
      // 命令菜单开着时：↑↓ 移高亮、回车选中当前项（选中后自动聚焦回输入框）
      const items = slashItemsRef.current;
      if (items.length > 0 && !composing) {
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
      if (isMobile && e.key === "Enter" && !e.shiftKey && !composing) {
        e.stopPropagation();
      }
    };
    ta.addEventListener("compositionstart", onCompStart);
    ta.addEventListener("compositionend", onCompEnd);
    ta.addEventListener("keydown", onKeyDownCapture, true);
    return () => {
      ta.removeEventListener("compositionstart", onCompStart);
      ta.removeEventListener("compositionend", onCompEnd);
      ta.removeEventListener("keydown", onKeyDownCapture, true);
    };
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
  /** 待上传的文件（可多选）。空数组＝没有待传，用它控制上传弹窗开合 */
  const [pendingFiles, setPendingFiles] = useState<File[]>([]);
  /** 批量上传进度：已完成数 / 总数，仅上传中有值 */
  const [uploadDone, setUploadDone] = useState(0);
  const [dirRel, setDirRel] = useState("");
  const [dirList, setDirList] = useState<string[]>([]);
  const [dirFiles, setDirFiles] = useState<string[]>([]);
  const [dirLoading, setDirLoading] = useState(false);
  const dirPollRef = useRef(0);
  // 「选择文件回填相对路径」模态（与上传共用目录浏览，但只读、点文件即插入路径）
  const [pickerOpen, setPickerOpen] = useState(false);
  /** 文件选择器里已勾选的相对路径（可跨子目录累积） */
  const [pickedRefs, setPickedRefs] = useState<string[]>([]);

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
  // 新建/重命名文件夹弹窗（应用内居中 Modal，替代原生 window.prompt）
  const [folderModal, setFolderModal] = useState<{ mode: "mkdir" | "rename"; orig: string } | null>(
    null,
  );
  const [folderInput, setFolderInput] = useState("");
  // 删除文件夹确认弹窗（替代原生 window.confirm）
  const [deleteTarget, setDeleteTarget] = useState<string | null>(null);

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
    setFolderInput("");
    setFolderModal({ mode: "mkdir", orig: "" });
  };

  const renameFolder = (name: string) => {
    setFolderInput(name);
    setFolderModal({ mode: "rename", orig: name });
  };

  const deleteFolder = (name: string) => setDeleteTarget(name);

  // 新建/重命名弹窗的确定：校验后下发，成功即关弹窗
  const submitFolder = () => {
    if (!folderModal) return;
    const next = folderInput.trim();
    if (!next) {
      message.warning("名称不能为空");
      return;
    }
    if (/[\\/]/.test(next)) {
      message.warning("名称不能包含斜杠");
      return;
    }
    if (folderModal.mode === "rename") {
      if (next !== folderModal.orig) runFsop("rename", folderModal.orig, next);
    } else {
      runFsop("mkdir", next);
    }
    setFolderModal(null);
  };

  const confirmDelete = () => {
    if (deleteTarget) runFsop("delete", deleteTarget);
    setDeleteTarget(null);
  };

  /**
   * 收下一批待上传文件。
   *
   * 弹窗已开时**追加**而不是替换：选完一批又想起还有几个，不该把前面选的顶掉。
   * 按「名字 + 大小」去重，挡住手滑重复选同一个文件。
   */
  const onPickFiles = (files: File[]) => {
    if (!files.length) {
      return;
    }
    if (!machineId || !cwd) {
      message.warning("该会话缺少设备或目录信息，无法传文件");
      return;
    }
    const first = pendingFiles.length === 0;
    setPendingFiles((prev) => {
      const seen = new Set(prev.map((f) => `${f.name}\u0000${f.size}`));
      return [...prev, ...files.filter((f) => !seen.has(`${f.name}\u0000${f.size}`))];
    });
    // 目录浏览状态只在「首次打开弹窗」时重置：追加文件不该把已经选好的目标目录清掉
    if (first) {
      setDirRel("");
      setDirList([]);
      setDirFiles([]);
      loadDirs("");
    }
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
  /**
   * 勾选/取消一个文件。存的是**完整相对路径**而不是文件名 ——
   * 选文件时可以来回进出子目录，只存名字的话跨目录同名文件会互相顶掉，
   * 而且插入时也无从知道它当初在哪一层。
   */
  const toggleFileRef = (name: string) => {
    const rel = `./${dirRel ? `${dirRel}/` : ""}${name}`;
    setPickedRefs((prev) =>
      prev.includes(rel) ? prev.filter((x) => x !== rel) : [...prev, rel],
    );
  };

  /** 把勾选的路径一次性插入输入框 */
  const insertPickedRefs = () => {
    if (pickedRefs.length) {
      appendToInput(pickedRefs.join(" "));
    }
    setPickedRefs([]);
    setPickerOpen(false);
  };

  // 拖拽中高亮
  const [dragOver, setDragOver] = useState(false);

  // 粘贴图片/文件 → 直接进上传流程（图片粘贴常无文件名，补个默认名）
  const onPaste = (e: React.ClipboardEvent) => {
    if (disabled) return;
    const items = Array.from(e.clipboardData?.items ?? []);
    // 剪贴板里可能一次带多个文件（比如在文件管理器里复制了几张图）
    const files = items
      .filter((it) => it.kind === "file")
      .map((it) => it.getAsFile())
      .filter((f): f is File => !!f);
    if (!files.length) return;
    e.preventDefault();
    // 截图粘贴出来的往往都叫 image.png，多个一起粘会重名互相覆盖 —— 加索引区分
    const named = files.map((f, i) =>
      f.name && f.name !== "image.png"
        ? f
        : new File(
            [f],
            `粘贴-${Date.now()}${files.length > 1 ? `-${i + 1}` : ""}.${
              f.type.split("/")[1] || "png"
            }`,
            { type: f.type },
          ),
    );
    onPickFiles(named);
  };

  /**
   * 依次上传选中的文件，成功的把相对路径一并填进输入框。
   *
   * 串行而非并发：一次可能选十几个文件，并发全推出去既容易把设备侧的写入撑爆，
   * 出错时也分不清是哪个失败的。串行慢一点，但每一步的成败都对得上号。
   * 单个失败不中断整批 —— 已经传上去的那些不该因为最后一个出错就白费。
   */
  const doUpload = async () => {
    const files = pendingFiles;
    if (!files.length || !machineId || !cwd) {
      return;
    }
    // 设备侧绝对目录 = 会话目录 + 相对子路径（按设备的分隔符拼）
    const sep = cwd.includes("\\") ? "\\" : "/";
    const dir = dirRel ? `${cwd}${sep}${dirRel.split("/").join(sep)}` : cwd;
    setUploading(true);
    setUploadDone(0);
    setPendingFiles([]);

    const ok: string[] = [];
    const failed: string[] = [];
    for (const file of files) {
      try {
        const res = await uploadPortalFile(machineId, dir, file);
        if (res.code === 0) {
          // 回填相对路径（相对会话目录，正斜杠通用）
          ok.push(dirRel ? `./${dirRel}/${file.name}` : `./${file.name}`);
        } else {
          failed.push(file.name);
        }
      } catch {
        failed.push(file.name);
      }
      setUploadDone((n) => n + 1);
    }

    setUploading(false);
    setUploadDone(0);
    // 一次性回填：逐个 append 会在输入框里触发多次光标跳动
    if (ok.length) {
      appendToInput(ok.join(" "));
    }
    if (failed.length) {
      message.error(
        `${failed.length} 个失败：${failed.slice(0, 3).join("、")}${
          failed.length > 3 ? " 等" : ""
        }`,
      );
    } else {
      message.success(ok.length > 1 ? `已上传 ${ok.length} 个文件` : "已上传");
    }
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
      message.warning(offHint);
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
        onPickFiles(Array.from(e.dataTransfer?.files ?? []));
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
          disabled ? offHint : "输入任务，回车发布"
        }
        onSend={guardedSend}
        uploadEnabled={false}
        buttonGroup={[
          // 历史放最左：它跟当前会话状态无关（会话没进程、没 cwd 时照样要能翻记录），
          // 所以不受下面那两个的 cwd && !disabled 条件限制
          {
            title: "查看会话历史（已结束会话的最终产出）",
            icon: (
              <HistoryOutlined
                className={styles.uploadIcon}
                style={{ fontSize: 17 }}
              />
            ),
            type: "text" as const,
            onClick: () => setHistoryOpen(true),
          },
          ...(cwd && !disabled
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
                  type: "text" as const,
                  onClick: openPicker,
                },
                ...(machineId
                  ? [
                      {
                        // 批量上传是串行的，会持续一段时间 —— 标题里带上进度，
                        // 否则用户只看到一个转圈的回形针，不知道传到第几个了
                        title: uploading
                          ? `正在上传… ${uploadDone} 个已完成`
                          : "传文件到会话目录（可多选，完成后自动填入路径）",
                        icon: <PaperClipOutlined className={styles.uploadIcon} />,
                        type: "text" as const,
                        loading: uploading,
                        onClick: () => fileRef.current?.click(),
                      },
                    ]
                  : []),
              ]
            : []),
        ]}
      />
      {/* 上传目录确认：默认会话所在目录，可改成设备上任意目录 */}
      <Modal
        className={styles.dirModal}
        title={
          pendingFiles.length > 1
            ? `传 ${pendingFiles.length} 个文件到设备`
            : "传文件到设备"
        }
        open={pendingFiles.length > 0}
        onCancel={() => setPendingFiles([])}
        onOk={doUpload}
        okText="上传"
        cancelText="取消"
        width={460}
        centered
      >
        <div className={styles.uploadForm}>
          <div className={styles.uploadFile}>
            {pendingFiles.length > 1 ? (
              // 多个时列出来并允许逐个剔除：多选常常手滑带上不想传的
              <div className={styles.uploadList}>
                {pendingFiles.map((f) => (
                  <div key={`${f.name} ${f.size}`} className={styles.uploadItem}>
                    <span className={styles.uploadItemName}>{f.name}</span>
                    <span
                      className={styles.uploadItemDel}
                      role="button"
                      tabIndex={0}
                      title="不传这个"
                      onClick={() =>
                        setPendingFiles((prev) => prev.filter((x) => x !== f))
                      }
                    >
                      ×
                    </span>
                  </div>
                ))}
              </div>
            ) : (
              <>
                文件：<b>{pendingFiles[0]?.name}</b>
              </>
            )}
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
            {pendingFiles.length > 1 ? (
              <>
                将上传到：<b>./{dirRel ? `${dirRel}/` : ""}</b>（{pendingFiles.length}{" "}
                个文件），路径会一并填进输入框
              </>
            ) : (
              <>
                将上传到：<b>./{dirRel ? `${dirRel}/` : ""}{pendingFiles[0]?.name}</b>
              </>
            )}
          </div>
        </div>
      </Modal>

      {/* 选择文件：浏览会话目录，点文件即把相对路径插入输入框（不上传） */}
      <Modal
        className={styles.dirModal}
        title={
          pickedRefs.length
            ? `选择文件（已选 ${pickedRefs.length} 个）`
            : "选择文件（插入相对路径）"
        }
        open={pickerOpen}
        onCancel={() => {
          setPickedRefs([]);
          setPickerOpen(false);
        }}
        // 多选要攒完再插，所以得有个确认出口 —— 原先点一下即插入并关闭，选不了第二个
        onOk={insertPickedRefs}
        okText={pickedRefs.length > 1 ? `插入 ${pickedRefs.length} 个路径` : "插入"}
        cancelText="取消"
        okButtonProps={{ disabled: pickedRefs.length === 0 }}
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
                {dirFiles.map((f) => {
                  const rel = `./${dirRel ? `${dirRel}/` : ""}${f}`;
                  const picked = pickedRefs.includes(rel);
                  return (
                    <div
                      key={`f-${f}`}
                      className={`${styles.dirItem} ${picked ? styles.dirItemPicked : ""}`}
                      role="button"
                      tabIndex={0}
                      aria-pressed={picked}
                      onClick={() => toggleFileRef(f)}
                    >
                      <span className={styles.dirIcon}>{picked ? "✅" : "📄"}</span> {f}
                    </div>
                  );
                })}
              </>
            )}
          </div>
          <div className={styles.uploadHint}>
            {pickedRefs.length ? (
              // 已选的列出来：可以进出多个子目录累积勾选，不显示的话就记不住选过哪些了
              <>已选：<b>{pickedRefs.join(" ")}</b></>
            ) : (
              <>点文件勾选，可跨目录多选，选完点「插入」</>
            )}
          </div>
        </div>
      </Modal>

      {/* 隐藏的文件选择器（由工具栏回形针按钮触发） */}
      <input
        ref={fileRef}
        type="file"
        multiple
        hidden
        onChange={(e) => {
          onPickFiles(Array.from(e.target.files ?? []));
          // 允许连续选同一个文件
          e.target.value = "";
        }}
      />

      {/* 新建 / 重命名文件夹（应用内居中弹窗，替代原生 window.prompt） */}
      <Modal
        title={folderModal?.mode === "rename" ? "重命名文件夹" : "新建文件夹"}
        open={!!folderModal}
        onCancel={() => setFolderModal(null)}
        onOk={submitFolder}
        okText="确定"
        cancelText="取消"
        okButtonProps={{ disabled: !folderInput.trim() }}
        width={400}
        centered
      >
        <Input
          value={folderInput}
          onChange={(value) => setFolderInput(value)}
          placeholder="文件夹名称"
          onPressEnter={submitFolder}
        />
      </Modal>

      {/* 删除文件夹确认（应用内居中弹窗，替代原生 window.confirm） */}
      <Modal
        title="删除文件夹"
        open={!!deleteTarget}
        onCancel={() => setDeleteTarget(null)}
        onOk={confirmDelete}
        okText="删除"
        cancelText="取消"
        okButtonProps={{ danger: true }}
        width={400}
        centered
      >
        <div>
          删除文件夹「{deleteTarget}」及其全部内容？此操作不可恢复。
        </div>
      </Modal>

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
        centered
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

      {/* 会话历史：已结束会话的最终产出 */}
      <HistoryModal
        open={historyOpen}
        onClose={() => setHistoryOpen(false)}
        taskId={taskId}
      />
    </div>
  );
};

export default Composer;
