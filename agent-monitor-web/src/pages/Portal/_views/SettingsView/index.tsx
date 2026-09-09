import React, { useEffect, useState } from "react";

import { Badge } from "antd";
import {
  BgColorsOutlined,
  CloseOutlined,
  CloudSyncOutlined,
  InfoCircleOutlined,
  LaptopOutlined,
  LeftOutlined,
  RightOutlined,
  RobotOutlined,
  SafetyOutlined,
  UserOutlined,
} from "@ant-design/icons";
import { observer } from "mobx-react-lite";
import { Outlet, useLocation, useNavigate } from "react-router-dom";

import { MOBILE_QUERY, isMobileViewport } from "@/utils/breakpoint";
import PortalStore from "../../PortalStore";
import { usePortalUser } from "../../_context/portalUser";
import {
  PORTAL_BASE,
  PORTAL_SETTINGS,
  SettingsTab,
  settingsPath,
} from "../../_utils/portalNav";
import styles from "./index.module.scss";

/**
 * 设置壳（`/portal/settings`）。
 *
 * 原来是一个 930 行的 `SettingsModal`，七个分栏靠 `useState` 的 tab 切换 ——
 * 代价是刷新丢失当前分栏、浏览器前进后退失效、「设备管理」这类页面发不出链接。
 * 现在七个分栏各有真实地址，本组件退回成一个壳：左侧导航 ＋ `<Outlet/>`。
 *
 * 桌面是左右分栏（导航常驻），移动端是 iOS 设置式两级：
 * `/portal/settings` 只显示一级菜单，点进去才是 `/portal/settings/<tab>`。
 * 这两态由**地址**决定，不再有 `mobileView` 状态。
 */
const SettingsView: React.FC = observer(() => {
  const navigate = useNavigate();
  const { pathname } = useLocation();
  const { pendingCount, loadDevices } = PortalStore;
  const [isMobile, setIsMobile] = useState(isMobileViewport);

  useEffect(() => {
    const mq = window.matchMedia(MOBILE_QUERY);
    const sync = () => setIsMobile(mq.matches);
    sync();
    mq.addEventListener("change", sync);
    return () => mq.removeEventListener("change", sync);
  }, []);

  // 设备角标要在任意分栏都准（导航常驻），所以在壳里拉一次
  useEffect(() => {
    loadDevices();
  }, [loadDevices]);

  const user = usePortalUser();

  // 每项原来还带一支 color（#3a8cff / #21b34a / #f2933c / #8a94a6），以内联
  // `--nav-color` 写在图标上 —— 但全项目没有任何一条 CSS 读这个变量：设计早已改成
  // 「单色细线图标」（见 index.module.scss 的 .navIcon）。
  const navItems: {
    key: SettingsTab;
    label: string;
    icon: React.ReactNode;
    badge?: number;
  }[] = [
    { key: "account", label: "账户", icon: <UserOutlined /> },
    { key: "appearance", label: "外观", icon: <BgColorsOutlined /> },
    { key: "devices", label: "设备管理", icon: <LaptopOutlined />, badge: pendingCount },
    { key: "configs", label: "配置同步", icon: <CloudSyncOutlined /> },
    { key: "bots", label: "机器人管理", icon: <RobotOutlined /> },
    { key: "security", label: "安全防护", icon: <SafetyOutlined /> },
    { key: "about", label: "关于", icon: <InfoCircleOutlined /> },
  ];

  const activeTab = navItems.find((n) =>
    pathname.startsWith(settingsPath(n.key)),
  )?.key;
  // 移动端一级页 = 地址正好停在 /portal/settings（没有分栏段）
  const atMenu = !activeTab;
  const currentLabel = navItems.find((n) => n.key === activeTab)?.label ?? "";

  const goBack = () => {
    // 移动端二级页返回一级菜单；一级菜单（与桌面任意分栏）返回会话页。
    // 与 Android 返回键、浏览器后退走的是同一条推导（_utils/portalNav）。
    navigate(isMobile && !atMenu ? PORTAL_SETTINGS : PORTAL_BASE);
  };

  return (
    <div className={`${styles.SettingsView} ${atMenu ? styles.atMenu : ""}`}>
      {/* 移动端固定头部：返回 + 标题（桌面隐藏，见 scss） */}
      <div className={styles.mobileHead}>
        {isMobile && !atMenu ? (
          <span
            className={styles.mobileBack}
            role="button"
            tabIndex={0}
            onClick={goBack}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                goBack();
              }
            }}
          >
            <LeftOutlined className={styles.mobileBackIcon} />
            设置
          </span>
        ) : null}
        <span className={styles.mobileHeadTitle}>
          {atMenu ? "设置" : currentLabel}
        </span>
      </div>

      {/* 关闭：回会话页。桌面在右上角，移动端是圆形悬浮钮（见 scss） */}
      <span
        className={styles.closeBtn}
        role="button"
        tabIndex={0}
        aria-label="关闭设置"
        onClick={() => navigate(PORTAL_BASE)}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            navigate(PORTAL_BASE);
          }
        }}
      >
        <CloseOutlined />
      </span>

      <aside className={styles.nav}>
        <div className={styles.navTitle}>设置</div>
        {/* Claude sheet 顶部的身份 pill（仅移动端，见 scss）：点按进账户 */}
        <div
          className={styles.identityPill}
          role="button"
          tabIndex={0}
          onClick={() => navigate(settingsPath("account"))}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              navigate(settingsPath("account"));
            }
          }}
        >
          <span className={styles.identityAvatar}>
            {(user.nickname ?? user.username ?? "U").slice(0, 1)}
          </span>
          <span className={styles.identityName}>
            {user.nickname ?? user.username}
          </span>
          <RightOutlined className={styles.identityArrow} />
        </div>
        <div className={styles.navList} role="tablist" aria-label="设置分类">
          {navItems.map((n) => (
            <div
              key={n.key}
              className={`${styles.navItem} ${
                activeTab === n.key ? styles.active : ""
              }`}
              role="tab"
              tabIndex={0}
              aria-selected={activeTab === n.key}
              onClick={() => navigate(settingsPath(n.key))}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  navigate(settingsPath(n.key));
                }
              }}
            >
              <span className={styles.navIcon}>{n.icon}</span>
              <span className={styles.navLabel}>{n.label}</span>
              {n.badge ? <Badge count={n.badge} size="small" /> : null}
              <RightOutlined className={styles.navChevron} />
            </div>
          ))}
        </div>
      </aside>

      <section className={styles.content}>
        <Outlet />
      </section>
    </div>
  );
});

export default SettingsView;
