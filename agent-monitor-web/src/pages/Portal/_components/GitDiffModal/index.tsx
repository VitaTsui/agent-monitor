import React, { useEffect, useRef, useState } from "react";

import { Modal } from "@hsu-react/ui";
import { Empty, Spin } from "antd";

import { GitOverview, getPortalGitDiff } from "@/services/apis/portal";
import styles from "./index.module.scss";

interface GitDiffModalProps {
  open: boolean;
  taskId: string;
  title?: string;
  onClose: () => void;
}

/** 一段 diff 拆成带高亮的行 */
interface DiffLine {
  type: "add" | "del" | "hunk" | "file" | "ctx";
  text: string;
}

function parseDiff(diff: string): DiffLine[] {
  return diff.split("\n").map((line) => {
    if (line.startsWith("diff --git") || line.startsWith("+++ ") || line.startsWith("--- ")) {
      return { type: "file" as const, text: line };
    }
    if (line.startsWith("@@")) return { type: "hunk" as const, text: line };
    if (line.startsWith("+")) return { type: "add" as const, text: line };
    if (line.startsWith("-")) return { type: "del" as const, text: line };
    return { type: "ctx" as const, text: line };
  });
}

/** git status --porcelain 的两位码 XY：按 git 语义取更有意义的那位翻译，覆盖全部码 */
const CODE_LABEL: Record<string, string> = {
  M: "改",
  A: "增",
  D: "删",
  R: "改名",
  C: "拷贝",
  U: "冲突",
  T: "类型变更",
  "?": "未跟踪",
  "!": "已忽略",
};

function statusLabel(status: string): string {
  const x = status[0] ?? " "; // 暂存区
  const y = status[1] ?? " "; // 工作区
  if (x === "U" || y === "U") return "冲突";
  // 优先展示工作区状态，其次暂存区
  const code = y !== " " && y !== undefined ? y : x;
  return CODE_LABEL[code] ?? (status.trim() || "改");
}

const GitDiffModal: React.FC<GitDiffModalProps> = (props) => {
  const { open, taskId, title, onClose } = props;
  const [loading, setLoading] = useState(false);
  const [overview, setOverview] = useState<GitOverview | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const tries = useRef(0);

  useEffect(() => {
    // alive 门闩：弹窗关闭/切换会话后，在途请求的回调一律丢弃，
    // 否则旧任务的 diff 会写进新任务的弹窗
    let alive = true;
    const stop = () => {
      alive = false;
      if (timer.current) clearTimeout(timer.current);
      timer.current = null;
    };
    if (!open || !taskId) {
      setOverview(null);
      return stop;
    }
    setLoading(true);
    setOverview(null);
    tries.current = 0;

    const fail = (msg: string) => {
      if (!alive) return;
      setLoading(false);
      setOverview({
        isRepo: false,
        branch: "",
        files: [],
        diff: "",
        untracked: [],
        error: msg,
      });
    };

    const poll = () => {
      getPortalGitDiff(taskId)
        .then((res) => {
          if (!alive) return;
          if (res.code !== 0) {
            fail(res.msg ?? "获取失败");
            return;
          }
          // 远程会话 pending：agent 还没回传，最多轮询 ~12s
          if (res.data?.pending && tries.current < 8) {
            tries.current += 1;
            timer.current = setTimeout(poll, 1500);
            return;
          }
          setLoading(false);
          setOverview(
            res.data?.overview ?? {
              isRepo: false,
              branch: "",
              files: [],
              diff: "",
              untracked: [],
              error: "该设备暂无响应，请稍后重试",
            },
          );
        })
        // 失败也要给出可见反馈，避免弹窗一片空白
        .catch(() => fail("请求失败，请稍后重试"));
    };
    poll();
    return stop;
  }, [open, taskId]);

  const lines = overview?.diff ? parseDiff(overview.diff) : [];

  return (
    <Modal
      className={styles.GitDiffModal}
      title={`改动对比${title ? ` · ${title}` : ""}`}
      open={open}
      onCancel={onClose}
      footer={null}
      width={860}
      centered
    >
      <Spin spinning={loading} tip="正在读取改动…">
        {overview && overview.error ? (
          <div className={styles.empty}>
            <Empty
              image={Empty.PRESENTED_IMAGE_SIMPLE}
              description={overview.error}
            />
          </div>
        ) : overview ? (
          <div className={styles.body}>
            <div className={styles.head}>
              <span className={styles.branch}>⎇ {overview.branch || "—"}</span>
              <span className={styles.count}>
                {overview.files.length + overview.untracked.length} 个改动
              </span>
            </div>

            {overview.files.length > 0 && (
              <div className={styles.fileList}>
                {overview.files.map((f) => (
                  <div key={f.path} className={styles.fileRow}>
                    <span className={styles.fileStatus} title={f.status}>
                      {statusLabel(f.status)}
                    </span>
                    <span className={styles.filePath}>{f.path}</span>
                  </div>
                ))}
              </div>
            )}

            {overview.untracked.length > 0 && (
              <div className={styles.untracked}>
                <div className={styles.untrackedTitle}>未跟踪的新文件</div>
                {overview.untracked.map((p) => (
                  <div key={p} className={styles.untrackedItem}>
                    ＋ {p}
                  </div>
                ))}
              </div>
            )}

            {lines.length > 0 ? (
              <pre className={styles.diff}>
                {lines.map((l, i) => (
                  <div key={i} className={`${styles.line} ${styles[l.type] ?? ""}`}>
                    {l.text || " "}
                  </div>
                ))}
              </pre>
            ) : overview.files.length === 0 && overview.untracked.length === 0 ? (
              <div className={styles.empty}>
                <Empty
                  image={Empty.PRESENTED_IMAGE_SIMPLE}
                  description="工作区干净，暂无未提交改动"
                />
              </div>
            ) : null}
          </div>
        ) : null}
      </Spin>
    </Modal>
  );
};

export default GitDiffModal;
