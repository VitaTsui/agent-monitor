import React, { useEffect, useState } from "react";

import { Input } from "@hsu-react/ui";
import { Badge, Dropdown, Tooltip } from "antd";
import {
  ControlOutlined,
  LaptopOutlined,
  LogoutOutlined,
  SafetyOutlined,
  SearchOutlined,
  SettingOutlined,
  UnorderedListOutlined,
  UpOutlined,
} from "@ant-design/icons";
import { observer } from "mobx-react-lite";
import { useNavigate } from "react-router-dom";

import { inNativeShell } from "@/utils/clientAuth";
import PortalStore from "../../PortalStore";
import DeviceSelect from "./_components/DeviceSelect";
import SessionTree from "./_components/SessionTree";
import type { PortalUserInfo } from "../../_context/portalUser";
import { HISTORY_LIST_PATH, type SettingsTab } from "../../_utils/portalNav";
import styles from "./index.module.scss";

/** 侧栏折叠图标：面板 + 左栏分隔线（对标 VS Code / ChatGPT 的侧栏切换，
 *  取代过于「后管菜单」的汉堡折叠图标）。折叠态把分隔线挪到更左，暗示会收窄。 */
export const SidebarIcon: React.FC<{ folded?: boolean }> = ({ folded }) => (
  <svg
    width="21"
    height="21"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    strokeWidth="1.9"
    strokeLinecap="round"
    strokeLinejoin="round"
    aria-hidden
  >
    <rect x="3" y="4.5" width="18" height="15" rx="2.6" />
    <line x1={folded ? "8" : "9.5"} y1="4.5" x2={folded ? "8" : "9.5"} y2="19.5" />
  </svg>
);

/** 搜索框敲完到真正发请求之间的等待。逐字发请求会把历史接口打成打字机 */
const SEARCH_DEBOUNCE_MS = 300;

interface SidebarProps {
  folded: boolean;
  onToggleFold: () => void;
  isMobile: boolean;
  /** 移动端顶栏是否在场（仅会话页）。在场时侧栏从顶栏下沿起，把那 48px 让出来 ——
   *  顶栏是半透明毛玻璃，侧栏留在底下会透出来跟顶栏的图标叠字。 */
  underTopBar: boolean;
  user: PortalUserInfo;
  userMenuOpen: boolean;
  onUserMenuOpenChange: (open: boolean) => void;
  /** 选中会话（移动端顺带收起抽屉；在子页面上顺带回会话页） */
  onSelectSession: (id: string) => void;
  /** 进设置的某个分栏 */
  onOpenSettings: (tab: SettingsTab) => void;
  /** 打开命令面板（⌘K）。快捷键挂在壳上，这里只是那颗按钮 */
  onOpenSearch: () => void;
  onLogout: () => void;
}

/** 前台左侧栏：品牌 ＋ 搜索 ＋ 客户端会话树 ＋ 底部账户。 */
const Sidebar: React.FC<SidebarProps> = observer((props) => {
  const {
    folded,
    onToggleFold,
    isMobile,
    underTopBar,
    user,
    userMenuOpen,
    onUserMenuOpenChange,
    onSelectSession,
    onOpenSettings,
    onOpenSearch,
    onLogout,
  } = props;
  const { pendingCount, keyword, setKeyword } = PortalStore;
  const navigate = useNavigate();

  const nickname = user.nickname ?? user.username ?? "";

  /* 输入框里的字与真正生效的关键字分开两份：前者每敲一下就变（不然输入框会卡顿），
     后者隔 300ms 才跟上（历史列表是要发请求的，逐字发等于把接口打成打字机）。 */
  const [draft, setDraft] = useState(keyword);
  useEffect(() => {
    const timer = window.setTimeout(
      () => setKeyword(draft),
      SEARCH_DEBOUNCE_MS,
    );
    return () => window.clearTimeout(timer);
  }, [draft, setKeyword]);

  /**
   * 账户菜单的条目。
   *
   * 从前是一个自绘的 `Popover` ＋ `.userMenu`（248 宽、自己写行高与分隔线）。
   * 换成 `Dropdown rootClassName="va-menu"` 之后，圆角 / 行高 / 图标槽 / 分隔线 /
   * 危险项配色全部由 `styles/antd-overload.scss` 里那份统一几何提供，
   * 这里只声明「有哪几项」。功能一项没增没减。
   */
  const accountMenu = [
    {
      key: "who",
      // 账号那一行本身不是一个动作，点它什么都不该发生
      disabled: true,
      label: (
        <span className={styles.menuWho}>
          <span className={styles.menuWhoName}>{user.username}</span>
          <span className={styles.menuWhoRole}>
            {user.isSuper ? "超级管理员" : "普通用户"}
          </span>
        </span>
      ),
    },
    { type: "divider" as const },
    {
      key: "settings",
      icon: <SettingOutlined />,
      label: "设置",
      onClick: () => onOpenSettings("account"),
    },
    {
      key: "devices",
      icon: <LaptopOutlined />,
      label: (
        <span className={styles.menuRow}>
          设备管理
          {pendingCount ? <Badge count={pendingCount} size="small" /> : null}
        </span>
      ),
      onClick: () => onOpenSettings("devices"),
    },
    {
      key: "security",
      icon: <SafetyOutlined />,
      label: "安全防护",
      onClick: () => onOpenSettings("security"),
    },
    // 后台管理只在浏览器里显示：客户端 / 移动端原生壳内隐藏（那里开新标签打不开后管）
    ...(user.isSuper && !inNativeShell()
      ? [
          {
            key: "admin",
            icon: <ControlOutlined />,
            label: "后台管理",
            onClick: () => {
              window.open("/admin", "_blank");
              onUserMenuOpenChange(false);
            },
          },
        ]
      : []),
    { type: "divider" as const },
    {
      key: "logout",
      icon: <LogoutOutlined />,
      label: "退出登录",
      danger: true,
      onClick: onLogout,
    },
  ];

  return (
    <aside
      className={`${styles.Sidebar} ${folded ? styles.folded : ""} ${
        underTopBar ? styles.underBar : ""
      }`}
    >
      {/* 顶部一行：字标把两颗按钮顶到右端（`.brandName { margin-right: auto }`）。
          高 44 / 左右 14 / **两颗之间 gap 2** —— 原来是 space-between，
          搜索和收起被推到一左一右，中间隔着半条侧栏那么宽。 */}
      <div className={styles.head}>
        {!folded && <span className={styles.brandName}>终端任务监控</span>}
        <Tooltip title="搜索（⌘K）" placement="right">
          <span
            className={styles.headBtn}
            role="button"
            tabIndex={0}
            aria-label="搜索"
            onClick={onOpenSearch}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                onOpenSearch();
              }
            }}
          >
            <SearchOutlined />
          </span>
        </Tooltip>
        <Tooltip title={folded ? "展开侧栏" : "收起侧栏"} placement="right">
          <span
            className={styles.headBtn}
            role="button"
            tabIndex={0}
            aria-label={folded ? "展开侧栏" : "收起侧栏"}
            aria-expanded={!folded}
            onClick={onToggleFold}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                onToggleFold();
              }
            }}
          >
            <SidebarIcon folded={folded} />
          </span>
        </Tooltip>
      </div>

      {!folded && (
        <>
          {/* 设备选择器。**独立成顶部的一行，不挤进上面那条工具条** ——
              那一行（字标 ＋ 搜索 ＋ 收起）是对整个侧栏的操作，44 / 0 14px / gap 2
              是照参照量过的，塞第四样东西进去就没法对齐了。
              设备是「下面这一列讲的是哪台机器」，它该在列表的正上方。 */}
          <div className={styles.deviceRow}>
            <DeviceSelect />
          </div>

          {/* 侧栏筛选框。这里曾经撤掉过一个同样位置的输入框 —— 理由是它只筛得动
              「当前选中设备」下的那一列，而顶部那颗搜索按钮（⌘K）搜的是全部。
              两个搜索摆在一起、结果却对不上，用户会以为是同一份东西。
              现在侧栏列的就是**全部客户端的全部会话**，这个框筛的也是同一份
              （关键字还会原样传给历史接口），两者不再是两套口径。 */}
          <div className={styles.searchRow}>
            <Input
              className={styles.searchInput}
              size="small"
              allowClear
              value={draft}
              placeholder="筛选会话 / 项目 / 主机"
              prefix={<SearchOutlined className={styles.searchIcon} />}
              onChange={setDraft}
            />
          </div>

          <div className={styles.siderScroll}>
            <SessionTree isMobile={isMobile} onSelect={onSelectSession} />

            {/* 列表末尾的「查看全部会话」。侧栏按客户端分组、每组翻页；
                跨设备、纯按最近活动排的全量在 /portal/history 那一页。
                做成与会话行同高的一行而不是一颗按钮 —— 它就是列表的最后一行。 */}
            <div
              className={styles.viewAll}
              role="button"
              tabIndex={0}
              onClick={() => navigate(HISTORY_LIST_PATH)}
              onKeyDown={(e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  navigate(HISTORY_LIST_PATH);
                }
              }}
            >
              <UnorderedListOutlined className={styles.viewAllIcon} />
              查看全部会话
            </div>
          </div>
        </>
      )}
      {folded && <div className={styles.siderScroll} />}

      {/* 底部账户（Claude / VitaAgent 式）：一行 32 高的按钮，点开是 va-menu 下拉 */}
      <div className={styles.foot}>
        <Dropdown
          rootClassName="va-menu"
          trigger={["click"]}
          placement="topLeft"
          open={userMenuOpen}
          onOpenChange={(o) => {
            // 移动端：不弹菜单，直接进整屏设置（Claude App 式头像入口）。
            // 不再顺手收起侧栏——点头像只是开设置，关掉后侧栏仍在。
            if (o && isMobile) {
              onOpenSettings("account");
              return;
            }
            onUserMenuOpenChange(o);
          }}
          menu={{ items: accountMenu }}
        >
          <button type="button" className={styles.account} title="账户与设置">
            <span className={styles.accountAvatar}>
              {nickname.slice(0, 1).toUpperCase() || "U"}
            </span>
            {!folded && (
              <>
                <span className={styles.accountName}>{nickname}</span>
                <Badge count={pendingCount} size="small">
                  <UpOutlined className={styles.accountCaret} />
                </Badge>
              </>
            )}
          </button>
        </Dropdown>
      </div>
    </aside>
  );
});

export default Sidebar;
