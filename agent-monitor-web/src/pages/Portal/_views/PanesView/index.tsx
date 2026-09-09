import React, { useEffect, useState } from "react";

import { CodeOutlined, SplitCellsOutlined } from "@ant-design/icons";
import { observer } from "mobx-react-lite";

import { MOBILE_QUERY, isMobileViewport } from "@/utils/breakpoint";
import PortalStore from "../../PortalStore";
import { usePortalUser } from "../../_context/portalUser";
import { usePaneGrid } from "../../_hooks/usePaneGrid";
import ChatPane from "../../_components/ChatPane";
import styles from "./index.module.scss";

/**
 * 会话面板区 —— `/portal` 的 index 视图。
 *
 * 从前它和侧栏、顶栏挤在同一个 786 行的 `Portal/index.tsx` 里，靠 `paneCount`
 * 三分支决定渲染哪一套。现在壳只留侧栏 ＋ 顶栏 ＋ `<Outlet/>`，这一块成为
 * 路由出口下的一个视图，和设置页/历史页同级。
 */
const PanesView: React.FC = observer(() => {
  const { openTasks, focusedId, setFocused } = PortalStore;
  const [isMobile, setIsMobile] = useState(isMobileViewport);

  useEffect(() => {
    const mq = window.matchMedia(MOBILE_QUERY);
    const sync = () => setIsMobile(mq.matches);
    sync();
    mq.addEventListener("change", sync);
    return () => mq.removeEventListener("change", sync);
  }, []);

  const paneCount = openTasks.length;
  // 放大的那一格。两道门槛：
  //   多格 —— 单格本来就占满，再「放大」没意义，还会渲染出「主区 + 空右列」；
  //   非移动端 —— 手机上主区要跟 320px 的右列分一块 390px 的屏，主区只剩个缝；
  //     更要命的是移动端把 paneHeader 整个隐藏了（见样式），进去就没有还原按钮，出不来。
  const focusedTask =
    paneCount > 1 && !isMobile
      ? openTasks.find((t) => t.id === focusedId)
      : undefined;
  // 横向还是纵向拆分、一行摆几个，全按网格容器的实际宽高算（见 usePaneGrid）
  const {
    ref: paneGridRef,
    cols: paneCols,
    rows: paneRows,
    lastSpan: paneLastSpan,
  } = usePaneGrid(paneCount);

  const user = usePortalUser();
  const nickname = user.nickname ?? user.username ?? "";

  if (paneCount === 0) {
    return (
      <div className={styles.mainEmpty}>
        <div className={styles.greeting}>
          <span className={styles.greetLogo}>
            <CodeOutlined />
          </span>
          你好，{nickname}
        </div>
        <div className={styles.greetSub}>
          <span className={styles.descDesktop}>从左侧选择一个终端会话查看执行内容</span>
          <span className={styles.descMobile}>点左上角菜单，选择一个终端会话查看</span>
        </div>
        <div className={`${styles.hint} ${styles.descDesktop}`}>
          点击会话右侧的 <SplitCellsOutlined /> 可并排显示多个任务
        </div>
      </div>
    );
  }

  if (focusedTask) {
    /* 放大模式：主区一格撑满，其余缩成右侧一列只读卡片。
       不走自适应网格 —— 那套是在「几格平分」的前提下算的，这里的诉求正相反：
       一格独大、其余只求瞥得见。 */
    return (
      <div className={styles.focusLayout}>
        <div className={styles.focusMain}>
          <ChatPane key={focusedTask.id} task={focusedTask} closable={paneCount > 1} />
        </div>
        <div className={styles.focusSide}>
          {openTasks
            .filter((t) => t.id !== focusedTask.id)
            .map((t) => (
              <div
                key={t.id}
                className={`${styles.focusCard} ${
                  t.pendingSelect?.questions?.length ? styles.focusCardAlert : ""
                }`}
              >
                <ChatPane
                  task={t}
                  closable={paneCount > 1}
                  compact
                  onActivate={() => setFocused(t.id ?? "")}
                />
              </div>
            ))}
        </div>
      </div>
    );
  }

  return (
    <div
      ref={paneGridRef}
      className={styles.paneGrid}
      data-last-span={paneLastSpan}
      style={
        {
          "--pane-cols": paneCols,
          "--pane-rows": paneRows,
        } as React.CSSProperties
      }
    >
      {openTasks.map((t) => (
        <ChatPane key={t.id} task={t} closable={paneCount > 1} />
      ))}
    </div>
  );
});

export default PanesView;
