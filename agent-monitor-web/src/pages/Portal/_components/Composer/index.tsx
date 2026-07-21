import React, { useEffect, useRef, useState } from "react";

import { Chat, Input, Modal } from "@hsu-react/ui";
import { message } from "antd";
import { reaction } from "mobx";
import {
  FileSearchOutlined,
  PaperClipOutlined,
  WarningOutlined,
} from "@ant-design/icons";

import {
  SlashCommand,
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
  const rootRef = useRef<HTMLDivElement>(null);
  const fileRef = useRef<HTMLInputElement>(null);

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

  // 监听输入框内容：以「/xxx」（无空格）开头就进命令模式并按 xxx 过滤
  useEffect(() => {
    const ta = rootRef.current?.querySelector("textarea");
    if (!ta) return;
    const onInput = () => {
      const v = ta.value;
      const m = /^\/(\S*)$/.exec(v);
      setSlashQuery(m ? m[1] : null);
    };
    ta.addEventListener("input", onInput);
    ta.addEventListener("blur", () => setTimeout(() => setSlashQuery(null), 150));
    return () => ta.removeEventListener("input", onInput);
  }, [taskId]);

  // 移动端：回车换行、不发送（发送用右下角发送按钮）。hsu-ui Chat.Input 默认回车即提交，
  // 这里在捕获阶段拦住移动端的 Enter、stopPropagation 阻止它到达 Chat.Input 的提交处理，
  // 不 preventDefault 让 textarea 自然插入换行。
  useEffect(() => {
    const ta = rootRef.current?.querySelector("textarea");
    if (!ta) return;
    const onKeyDownCapture = (e: KeyboardEvent) => {
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
    }
    setSlashQuery(null);
  };

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
              className={`${styles.slashItem} ${styles.slashCustom}`}
              role="button"
              tabIndex={0}
              onMouseDown={(e) => e.preventDefault()}
              onClick={sendCustom}
            >
              <span className={styles.slashName}>发送 /{slashQuery}</span>
              <span className={styles.slashDesc}>不在列表中的自定义命令，直接发出</span>
            </div>
          ) : null}
          {matched.map((c) => (
            <div
              key={c.name}
              className={styles.slashItem}
              role="button"
              tabIndex={0}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => fillCommand(c.name)}
            >
              <span className={styles.slashName}>{c.name}</span>
              {c.desc ? <span className={styles.slashDesc}>{c.desc}</span> : null}
            </div>
          ))}
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
          <div className={styles.uploadLabel}>目标目录（会话目录内选择）</div>
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
                <div
                  key={d}
                  className={styles.dirItem}
                  role="button"
                  tabIndex={0}
                  onClick={() => enterDir(d)}
                >
                  <span className={styles.dirIcon}>📁</span> {d}
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
