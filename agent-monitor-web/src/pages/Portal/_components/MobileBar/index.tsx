import React, { useState } from "react";

import { Modal, Popover } from "antd";
import {
  EllipsisOutlined,
  PauseCircleOutlined,
  PlayCircleOutlined,
  StopOutlined,
  SyncOutlined,
  ThunderboltOutlined,
} from "@ant-design/icons";
import { observer } from "mobx-react-lite";

import PortalStore from "../../PortalStore";
import { sessionTitle } from "../../_utils/sessionNote";
import { SidebarIcon } from "../Sidebar";
import SessionRename from "../SessionRename";
import styles from "./index.module.scss";

interface MobileBarProps {
  /** 抽屉是否展开（决定汉堡图标形态） */
  navOpen: boolean;
  onToggleNav: () => void;
}

/**
 * 移动端顶栏（仅窄屏显示，见 scss）。只在**会话页**渲染 —— 子路由页
 * （设置 / 历史）自带固定头部，两个头叠在一起就成了双层导航栏。
 */
const MobileBar: React.FC<MobileBarProps> = observer((props) => {
  const { navOpen, onToggleNav } = props;
  const { openTasks, control, syncMessages } = PortalStore;
  const [actsOpen, setActsOpen] = useState(false);

  const t0 = openTasks[0];

  return (
    <div className={styles.mobileBar}>
      <span
        className={styles.mobileMenuBtn}
        role="button"
        tabIndex={0}
        aria-label="打开/收起会话列表"
        onClick={onToggleNav}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            onToggleNav();
          }
        }}
      >
        <SidebarIcon folded={!navOpen} />
      </span>
      <span className={styles.mobileTitle}>
        {t0 ? (
          <span
            className={`${styles.mobileStatusDot} ${styles[t0.status ?? ""] ?? ""}`}
          />
        ) : null}
        {/* 移动端会话头部整个被样式隐藏，这里是标题唯一露面的地方 ——
            改名入口也只能在这儿：点标题即改。 */}
        {t0 ? (
          <SessionRename
            taskId={t0.id ?? ""}
            note={t0.note}
            disabled={!t0.id}
            className={styles.mobileTitleEdit}
          >
            <span className={styles.mobileTitleText}>
              {sessionTitle(t0, "终端任务监控")}
            </span>
          </SessionRename>
        ) : (
          "终端任务监控"
        )}
      </span>
      {/* 正在跑的时候「中断」提到一级：手机上想停一下是最急的操作，
          埋在 ⋯ 里要点两次、还要在小菜单里瞄准。不跑时不占位。 */}
      {t0 && t0.status === "running" && t0.pid ? (
        <span
          className={styles.mobileStopBtn}
          role="button"
          aria-label="中断当前任务"
          onClick={() => control(t0.id ?? "", "interrupt")}
        >
          <ThunderboltOutlined />
        </span>
      ) : null}
      {t0 ? (
        <Popover
          open={actsOpen}
          onOpenChange={setActsOpen}
          trigger="click"
          placement="bottomRight"
          arrow={false}
          overlayClassName={styles.mobileActsPop}
          content={
            <div className={styles.mobileActsMenu}>
              {(() => {
                const id0 = t0.id ?? "";
                const paused = t0.status === "paused";
                // 空闲时中断没有可断的东西，点了看不出任何变化，人会反复戳 ——
                // 等下一轮真跑起来时那几下反而把新任务打断了（同 ChatPane）
                const canInterrupt = !!t0.pid && t0.status === "running";
                const act = (fn: () => void) => () => {
                  setActsOpen(false);
                  fn();
                };
                return (
                  <>
                    <div
                      className={styles.mobileActItem}
                      onClick={act(() => syncMessages(id0))}
                    >
                      <SyncOutlined /> 重新同步
                    </div>
                    <div
                      className={styles.mobileActItem}
                      onClick={act(() => control(id0, paused ? "resume" : "pause"))}
                    >
                      {paused ? <PlayCircleOutlined /> : <PauseCircleOutlined />}
                      {paused ? " 恢复" : " 暂停"}
                    </div>
                    <div
                      className={`${styles.mobileActItem} ${
                        canInterrupt ? "" : styles.disabled
                      }`}
                      onClick={canInterrupt ? act(() => control(id0, "interrupt")) : undefined}
                    >
                      <ThunderboltOutlined /> 中断
                      {/* 条目名保持动作，不可用的缘由另起一行小字 ——
                          把状态描述塞进名字里，读着就不像个能点的东西 */}
                      {canInterrupt ? null : (
                        <span className={styles.actWhy}>当前没在执行</span>
                      )}
                    </div>
                    {/* 终止 = 杀进程，这一轮的上下文就没了。桌面端一直有二次确认，
                        移动端却是一点就执行 —— 而手指在紧挨着的菜单项上更容易滑错。 */}
                    <div
                      className={`${styles.mobileActItem} ${styles.danger}`}
                      onClick={act(() =>
                        Modal.confirm({
                          title: "确定终止该任务进程？",
                          content: "终端里这一轮的上下文会一起结束，无法恢复。",
                          okText: "终止",
                          cancelText: "取消",
                          okButtonProps: { danger: true },
                          onOk: () => control(id0, "stop"),
                        }),
                      )}
                    >
                      <StopOutlined /> 终止进程
                    </div>
                  </>
                );
              })()}
            </div>
          }
        >
          <span className={styles.mobileMoreBtn} role="button" aria-label="会话操作">
            <EllipsisOutlined />
          </span>
        </Popover>
      ) : null}
    </div>
  );
});

export default MobileBar;
