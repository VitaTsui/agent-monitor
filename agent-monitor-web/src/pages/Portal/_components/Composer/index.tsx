import React, { useEffect, useRef, useState } from "react";

import { Chat, Input, Modal } from "@hsu-react/ui";
import { Tooltip, message } from "antd";
import { PaperClipOutlined, WarningOutlined } from "@ant-design/icons";

import {
  SlashCommand,
  getPortalSlashCommands,
  uploadPortalFile,
} from "@/services/apis/portal";
import { CONFIRM_WORD, DangerHit, checkDanger } from "../../_utils/dangerCheck";
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
  const [cmdsOpen, setCmdsOpen] = useState(false);
  const [uploading, setUploading] = useState(false);
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

  // 选中的待上传文件 + 目标目录（默认会话所在目录，可改）
  const [pendingFile, setPendingFile] = useState<File | null>(null);
  const [uploadDir, setUploadDir] = useState("");

  const onPickFile = (file: File) => {
    if (!machineId || !cwd) {
      message.warning("该会话缺少设备或目录信息，无法传文件");
      return;
    }
    setPendingFile(file);
    setUploadDir(cwd);
  };

  /** 确认上传到指定目录，成功后把路径填入输入框 */
  const doUpload = () => {
    const file = pendingFile;
    const dir = uploadDir.trim();
    if (!file || !machineId || !dir) {
      return;
    }
    setUploading(true);
    setPendingFile(null);
    uploadPortalFile(machineId, dir, file)
      .then((res) => {
        if (res.code === 0) {
          message.success(res.data?.result ?? "已上传");
          // 传到会话目录用相对路径，其他目录用完整路径
          const sep = dir.includes("\\") ? "\\" : "/";
          appendToInput(
            dir === cwd ? `./${file.name}` : `${dir}${sep}${file.name}`,
          );
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

  const shownCommands = cmdsOpen ? commands : commands.slice(0, 6);

  return (
    <div className={styles.Composer} ref={rootRef}>
      {commands.length > 0 && !disabled && (
        <div className={styles.commands}>
          {shownCommands.map((c) => (
            <Tooltip key={c.name} title={c.desc}>
              <span
                className={styles.cmdChip}
                role="button"
                tabIndex={0}
                onClick={() => guardedSend(c.name)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" || e.key === " ") {
                    e.preventDefault();
                    guardedSend(c.name);
                  }
                }}
              >
                {c.name}
              </span>
            </Tooltip>
          ))}
          {commands.length > 6 && (
            <span
              className={styles.cmdMore}
              role="button"
              tabIndex={0}
              onClick={() => setCmdsOpen(!cmdsOpen)}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  setCmdsOpen(!cmdsOpen);
                }
              }}
            >
              {cmdsOpen ? "收起" : `+${commands.length - 6}`}
            </span>
          )}
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
          machineId && cwd && !disabled
            ? [
                {
                  title: "传文件到会话目录（完成后自动填入路径）",
                  icon: <PaperClipOutlined className={styles.uploadIcon} />,
                  type: "text",
                  loading: uploading,
                  onClick: () => fileRef.current?.click(),
                },
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
          <div className={styles.uploadLabel}>目标目录（默认会话所在目录）</div>
          <Input
            value={uploadDir}
            onChange={(v) => setUploadDir(v)}
            placeholder="设备上的目标目录"
          />
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
