import React from "react";

import { Input } from "@hsu-react/ui";
import { Badge, Popover, Tooltip } from "antd";
import {
  CodeOutlined,
  DownOutlined,
  SearchOutlined,
  UnorderedListOutlined,
} from "@ant-design/icons";
import { observer } from "mobx-react-lite";
import { useNavigate } from "react-router-dom";

import PortalStore from "../../PortalStore";
import UserMenu from "../UserMenu";
import DeviceList from "./_components/DeviceList";
import SessionList from "./_components/SessionList";
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

interface SidebarProps {
  folded: boolean;
  onToggleFold: () => void;
  isMobile: boolean;
  /** 客户端窗口内的本机 machineId（浏览器里为 null，不标「本机」） */
  localId: string | null;
  user: PortalUserInfo;
  userMenuOpen: boolean;
  onUserMenuOpenChange: (open: boolean) => void;
  /** 选中会话（移动端顺带收起抽屉；在子页面上顺带回会话页） */
  onSelectSession: (id: string) => void;
  /** 进设置的某个分栏 */
  onOpenSettings: (tab: SettingsTab) => void;
  onLogout: () => void;
}

/** 前台左侧栏：品牌 ＋ 搜索 ＋ 设备 ＋ 会话 ＋ 底部账户。 */
const Sidebar: React.FC<SidebarProps> = observer((props) => {
  const {
    folded,
    onToggleFold,
    isMobile,
    localId,
    user,
    userMenuOpen,
    onUserMenuOpenChange,
    onSelectSession,
    onOpenSettings,
    onLogout,
  } = props;
  const { pendingCount, keyword, setKeyword } = PortalStore;
  const navigate = useNavigate();

  const nickname = user.nickname ?? user.username ?? "";

  return (
    <aside className={`${styles.Sidebar} ${folded ? styles.folded : ""}`}>
      <div className={styles.siderHeader}>
        <div className={styles.brand}>
          <span className={styles.logo}>
            <CodeOutlined />
          </span>
          {!folded && <span className={styles.brandName}>终端任务监控</span>}
        </div>
        <Tooltip title={folded ? "展开侧栏" : "收起侧栏"} placement="right">
          <span
            className={styles.foldBtn}
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
          <div className={styles.siderSearch}>
            <Input
              className={styles.search}
              placeholder="搜索会话 / 项目"
              prefix={<SearchOutlined className={styles.searchIcon} />}
              allowClear
              value={keyword}
              onChange={(value) => setKeyword(value)}
            />
          </div>

          <div className={styles.siderScroll}>
            <div className={styles.sectionLabel}>设备</div>
            <DeviceList localId={localId} />

            <div className={styles.sectionLabel}>会话</div>
            <SessionList isMobile={isMobile} onSelect={onSelectSession} />

            {/* 列表末尾的「查看全部会话」。侧栏按设备/项目分组，只看得到当前
                选中设备下的那些；跨设备的全量在 /portal/history 那一页。
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

      {/* 底部用户区（Claude 式） */}
      <Popover
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
        content={
          <UserMenu
            user={user}
            pendingCount={pendingCount}
            onOpenSettings={onOpenSettings}
            onLogout={onLogout}
            onClose={() => onUserMenuOpenChange(false)}
          />
        }
        trigger="click"
        placement="topLeft"
        arrow={false}
        overlayClassName={styles.userMenuOverlay}
      >
        {/* Popover 的 click 触发挂在本元素注入的 onClick 上，键盘路径合成一次
            click 即可复用；不加 role/tabIndex 的话，设置/设备管理/安全防护/
            后台管理/退出登录全部无法用键盘抵达（它们没有别的入口）。 */}
        <div
          className={styles.userRow}
          title="账户与设置"
          role="button"
          tabIndex={0}
          aria-label="账户与设置"
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              e.currentTarget.click();
            }
          }}
        >
          <span className={styles.userAvatar}>{nickname.slice(0, 1) || "U"}</span>
          {!folded && (
            <>
              <div className={styles.userText}>
                <div className={styles.userName}>{nickname}</div>
              </div>
              <Badge count={pendingCount} size="small">
                <DownOutlined className={styles.userChevron} />
              </Badge>
            </>
          )}
        </div>
      </Popover>
    </aside>
  );
});

export default Sidebar;
