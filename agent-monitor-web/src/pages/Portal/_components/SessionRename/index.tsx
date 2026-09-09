import React, { useEffect, useState } from "react";

import { Button, Input } from "@hsu-react/ui";
import { Popover } from "antd";
import { EditOutlined } from "@ant-design/icons";

import PortalStore from "../../PortalStore";
import { NOTE_MAX_CHARS, noteLength } from "../../_utils/sessionNote";
import styles from "./index.module.scss";

interface SessionRenameProps {
  /** 要改名的会话 */
  taskId: string;
  /** 当前备注；没起过名字为空 —— 决定按钮是「保存」还是「清除」 */
  note?: string | null;
  /** 点上去会弹出改名框的那个标题节点 */
  children: React.ReactNode;
  /**
   * 只读：紧凑卡片整卡可点（换到主区），标题再抢一个点击就没法用了；
   * 会话 id 缺失时同理，没有可改的对象。
   */
  disabled?: boolean;
  className?: string;
}

/**
 * 会话改名：点标题就地弹出一个输入框。
 *
 * 为什么不做成路由页（远程往来是 `/portal/history/:taskId`）：
 * 那次重构解决的是「刷新丢当前页、后退失效、链接发不出去」，针对的是有内容、要分享、
 * 要能刷新保活的**界面**。改名是一次性的即时动作，给它一条 `/portal/rename/:id`，等于
 * 把一个输入框做成可分享链接，回头还要处理「照着链接进来但会话已经没了」。
 *
 * 也不用全屏弹窗：名字就写在标题上，在它原地改最直观，且桌面与移动端能共用同一套 ——
 * 移动端会话头部整个被隐藏，标题只在顶栏那一处，正好也是点它就改。
 */
const SessionRename: React.FC<SessionRenameProps> = (props) => {
  const { taskId, note, children, disabled, className } = props;
  const [open, setOpen] = useState(false);
  const [text, setText] = useState("");
  const [saving, setSaving] = useState(false);

  // 每次打开都从当前备注起步：上次改了一半没保存的残留不该留到下一次
  useEffect(() => {
    if (open) {
      setText(note ?? "");
    }
  }, [open, note]);

  const len = noteLength(text);
  const tooLong = len > NOTE_MAX_CHARS;
  // 清空保存 = 清除备注，恢复自动标题
  const clearing = len === 0;

  const save = async () => {
    if (tooLong || saving) {
      return;
    }
    if (clearing && !note) {
      // 本来就没名字，还提交个空串 —— 什么都没发生，直接收起来
      setOpen(false);
      return;
    }
    setSaving(true);
    const ok = await PortalStore.setNote(taskId, text);
    setSaving(false);
    // 失败不收起：错误提示已经弹了，输入内容留着让用户改，别逼他重打一遍
    if (ok) {
      setOpen(false);
    }
  };

  if (disabled) {
    return <>{children}</>;
  }

  return (
    <Popover
      open={open}
      onOpenChange={setOpen}
      trigger="click"
      placement="bottomLeft"
      arrow={false}
      overlayClassName={styles.renamePop}
      content={
        <div className={styles.renameForm}>
          <div className={styles.renameTitle}>会话备注</div>
          <Input
            autoFocus
            value={text}
            placeholder="给这个会话起个名字"
            // 超长不截断（截了用户以为存进去了），标红计数 + 禁用保存当场说明白
            count={{ show: true, max: NOTE_MAX_CHARS }}
            onChange={setText}
            onPressEnter={save}
          />
          <div className={`${styles.renameHint} ${tooLong ? styles.over : ""}`}>
            {tooLong
              ? `最长 ${NOTE_MAX_CHARS} 个字，当前 ${len} 个`
              : "名字跟着终端窗口走，/clear 后仍在；清空保存即恢复自动标题"}
          </div>
          <div className={styles.renameActions}>
            <Button size="small" type="text" onClick={() => setOpen(false)}>
              取消
            </Button>
            <Button
              size="small"
              type="primary"
              loading={saving}
              disabled={tooLong}
              onClick={save}
            >
              {clearing && note ? "清除" : "保存"}
            </Button>
          </div>
        </div>
      }
    >
      <span
        className={`${styles.renameTrigger} ${className ?? ""}`}
        role="button"
        tabIndex={0}
        aria-label="重命名会话"
        title="点击重命名"
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            setOpen(true);
          }
        }}
      >
        {children}
        <EditOutlined className={styles.renameIcon} />
      </span>
    </Popover>
  );
};

export default SessionRename;
