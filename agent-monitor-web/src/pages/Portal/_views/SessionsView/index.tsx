import React from "react";

import { Empty } from "antd";
import { CodeOutlined, LeftOutlined } from "@ant-design/icons";
import { observer } from "mobx-react-lite";
import { useNavigate } from "react-router-dom";

import PortalStore from "../../PortalStore";
import { PORTAL_BASE } from "../../_utils/portalNav";
import { sessionTitle } from "../../_utils/sessionNote";
import views from "../views.module.scss";
import styles from "./index.module.scss";

/** 状态 → 状态点的样式类。与侧栏会话行、会话面板同一口径。 */
const DOT_CLASS: Record<string, string> = {
  running: styles.running,
  idle: styles.idle,
  paused: styles.paused,
  finished: styles.finished,
};

/**
 * 「多久以前」。列表右端要的是「离现在多远」，不是精确时刻 ——
 * 精确时刻在会话里看，这里回答的是「哪些还热着」。
 */
const sinceNow = (iso: string | null | undefined): string => {
  if (!iso) return "";
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "";
  const min = Math.floor((Date.now() - t) / 60000);
  if (min < 1) return "刚刚";
  if (min < 60) return `${min} 分钟前`;
  const hour = Math.floor(min / 60);
  if (hour < 24) return `${hour} 小时前`;
  const day = Math.floor(hour / 24);
  if (day < 30) return `${day} 天前`;
  return new Date(t).toLocaleDateString("zh-CN");
};

/**
 * 全部会话（`/portal/history`）。侧栏那条「查看全部会话」的落点。
 *
 * 侧栏按「设备 → 项目」分组，一次只看得到当前选中设备下的那些；跨设备的全量
 * 在这一页。**不另调接口** —— 会话列表本来就由壳经 `GET /monitor/tasks` 拉齐、
 * 再由 WS 推着更新（见 PortalStore），这里只是换一种排法把同一份数据铺开：
 * 纯按最近活动倒序，回答「我最近在弄什么」。
 */
const SessionsView: React.FC = observer(() => {
  const navigate = useNavigate();
  const { tasks } = PortalStore;

  const items = [...tasks].sort((a, b) =>
    String(b.lastActiveAt ?? b.startedAt ?? "").localeCompare(
      String(a.lastActiveAt ?? a.startedAt ?? ""),
    ),
  );

  const open = (id: string) => {
    PortalStore.select(id);
    navigate(PORTAL_BASE);
  };

  return (
    <div className={views.pageFixed}>
      <div>
        <div className={views.fixedHead}>
          <div className={views.headRow}>
            <span
              className={views.headBtn}
              role="button"
              tabIndex={0}
              aria-label="返回会话"
              onClick={() => navigate(PORTAL_BASE)}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  navigate(PORTAL_BASE);
                }
              }}
            >
              <LeftOutlined />
            </span>
            <span className={views.headTitle}>全部会话</span>
            <span className={styles.count}>{items.length}</span>
          </div>
        </div>

        <div className={views.fixedBody}>
          {items.length === 0 ? (
            <Empty description="还没有会话" />
          ) : (
            <div className={styles.list}>
              {items.map((t) => {
                const id = t.id ?? "";
                return (
                  <div
                    key={id}
                    className={styles.row}
                    role="button"
                    tabIndex={0}
                    onClick={() => open(id)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" || e.key === " ") {
                        e.preventDefault();
                        open(id);
                      }
                    }}
                  >
                    <span className={styles.rowMain}>
                      <span
                        className={`${styles.dot} ${
                          DOT_CLASS[t.status ?? ""] ?? styles.finished
                        }`}
                      />
                      <CodeOutlined className={styles.rowIcon} />
                      <span className={styles.rowTitle}>
                        {sessionTitle(t, "新会话")}
                      </span>
                    </span>
                    <span className={styles.rowMeta}>
                      <span className={styles.rowHost}>{t.hostname}</span>
                      <span className={styles.rowStatus}>{t.statusDsr}</span>
                      <span className={styles.rowTime}>
                        {sinceNow(t.lastActiveAt)}
                      </span>
                    </span>
                  </div>
                );
              })}
            </div>
          )}
        </div>
      </div>
    </div>
  );
});

export default SessionsView;
