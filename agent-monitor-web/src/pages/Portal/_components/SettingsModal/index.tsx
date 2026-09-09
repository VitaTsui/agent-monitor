import React, { Suspense, lazy, useEffect } from "react";

import { Modal } from "@hsu-react/ui";
import { Badge, Spin } from "antd";
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

import PortalStore from "../../PortalStore";
import { usePortalUser } from "../../_context/portalUser";
import type { SettingsTab } from "../../_utils/portalNav";
import styles from "./index.module.scss";

/**
 * 七个分栏各自懒加载。
 *
 * 弹窗本体（这个壳）跟着首屏进包，但「配置同步」「机器人接入」「设备管理」
 * 那几块是首屏用不到的重块 —— 静态引进来会跟会话页一起进主包。
 * hsu-ui 的 Modal 在第一次打开前不挂载 children，所以这些切块也就不会提前拉。
 */
const AccountPane = lazy(() => import("./panes/AccountPane"));
const AppearancePane = lazy(() => import("./panes/AppearancePane"));
const DevicesPane = lazy(() => import("./panes/DevicesPane"));
const ConfigsPane = lazy(() => import("./panes/ConfigsPane"));
const BotsPane = lazy(() => import("./panes/BotsPane"));
const SecurityPane = lazy(() => import("./panes/SecurityPane"));
const AboutPane = lazy(() => import("./panes/AboutPane"));

const PANES: Record<SettingsTab, React.LazyExoticComponent<React.FC>> = {
  account: AccountPane,
  appearance: AppearancePane,
  devices: DevicesPane,
  configs: ConfigsPane,
  bots: BotsPane,
  security: SecurityPane,
  about: AboutPane,
};

interface SettingsModalProps {
  open: boolean;
  /**
   * 当前分栏。`null` = 移动端停在一级菜单（桌面没有这一态，右边不能空着，
   * 按「账户」渲染）。这份状态由壳（Portal/index.tsx）持有 —— Android 返回键
   * 要靠它判断「退到一级菜单」还是「关掉弹窗」，两处各存一份就会不同步。
   */
  tab: SettingsTab | null;
  isMobile: boolean;
  onTabChange: (tab: SettingsTab | null) => void;
  onClose: () => void;
}

/**
 * 设置弹窗。桌面是 960 宽的居中弹窗（左栏 190 ＋ 内容列 720），
 * 移动端由 styles/antd-overload.scss 统一变成底部 sheet，内部再分 iOS 式两级。
 */
const SettingsModal: React.FC<SettingsModalProps> = observer((props) => {
  const { open, tab, isMobile, onTabChange, onClose } = props;
  const { pendingCount, loadDevices } = PortalStore;
  const user = usePortalUser();

  // 设备角标要在任意分栏都准（左栏常驻），所以在壳里拉一次
  useEffect(() => {
    if (open) {
      loadDevices();
    }
  }, [open, loadDevices]);

  const navItems: {
    key: SettingsTab;
    label: string;
    icon: React.ReactNode;
    badge?: number;
  }[] = [
    { key: "account", label: "账户", icon: <UserOutlined /> },
    { key: "appearance", label: "外观", icon: <BgColorsOutlined /> },
    {
      key: "devices",
      label: "设备管理",
      icon: <LaptopOutlined />,
      badge: pendingCount,
    },
    { key: "configs", label: "配置同步", icon: <CloudSyncOutlined /> },
    { key: "bots", label: "机器人管理", icon: <RobotOutlined /> },
    { key: "security", label: "安全防护", icon: <SafetyOutlined /> },
    { key: "about", label: "关于", icon: <InfoCircleOutlined /> },
  ];

  // 移动端一级菜单 = 没选分栏；桌面没有这一态，右边落到「账户」
  const atMenu = isMobile && !tab;
  const activeTab: SettingsTab = tab ?? "account";
  const Pane = PANES[activeTab];
  const currentLabel =
    navItems.find((n) => n.key === activeTab)?.label ?? "设置";

  // 键盘可达：导航项是 div（要挂 role="tab"），Enter/空格等同点击
  const onKeyActivate = (fn: () => void) => (e: React.KeyboardEvent) => {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      fn();
    }
  };

  return (
    <Modal
      className={styles.SettingsModal}
      open={open}
      onCancel={onClose}
      footer={null}
      // 不给 Modal 标题：那行标题会横跨整个弹窗，把左右两栏整体压低一截
      title={null}
      // 960 = 左栏 190 ＋ 1px 分隔线 ＋ 内容列 720 与左右各 24 的内边距
      width={960}
      centered
      destroyOnHidden={false}
    >
      <div className={`${styles.body} ${atMenu ? styles.atMenu : ""}`}>
        {/* 移动端固定头部：返回 / 标题 / 关闭（桌面隐藏，见 scss） */}
        <div className={styles.mobileHead}>
          {atMenu ? null : (
            <span
              className={styles.mobileBack}
              role="button"
              tabIndex={0}
              onClick={() => onTabChange(null)}
              onKeyDown={onKeyActivate(() => onTabChange(null))}
            >
              <LeftOutlined className={styles.mobileBackIcon} />
              设置
            </span>
          )}
          <span className={styles.mobileHeadTitle}>
            {atMenu ? "设置" : currentLabel}
          </span>
          <span
            className={styles.mobileClose}
            role="button"
            tabIndex={0}
            aria-label="关闭设置"
            onClick={onClose}
            onKeyDown={onKeyActivate(onClose)}
          >
            <CloseOutlined />
          </span>
        </div>

        <aside className={styles.nav}>
          <div className={styles.navTitle}>设置</div>
          {/* Claude sheet 顶部的身份 pill（仅移动端，见 scss）：点按进账户 */}
          <div
            className={styles.identityPill}
            role="button"
            tabIndex={0}
            onClick={() => onTabChange("account")}
            onKeyDown={onKeyActivate(() => onTabChange("account"))}
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
                  !atMenu && activeTab === n.key ? styles.active : ""
                }`}
                role="tab"
                tabIndex={0}
                aria-selected={!atMenu && activeTab === n.key}
                onClick={() => onTabChange(n.key)}
                onKeyDown={onKeyActivate(() => onTabChange(n.key))}
              >
                <span className={styles.navIcon}>{n.icon}</span>
                <span className={styles.navLabel}>{n.label}</span>
                {n.badge ? <Badge count={n.badge} size="small" /> : null}
                <RightOutlined className={styles.navChevron} />
              </div>
            ))}
          </div>
        </aside>

        <section className={styles.panel}>
          {/* 懒加载边界必须在这里：切分栏是点击触发的同步更新，没有边界的话
              React 18 会把挂起的那棵树整个丢掉（表现为弹窗内容瞬间空白） */}
          <Suspense
            fallback={
              <div className={styles.panelLoading}>
                <Spin />
              </div>
            }
          >
            <Pane />
          </Suspense>
        </section>
      </div>
    </Modal>
  );
});

export default SettingsModal;
