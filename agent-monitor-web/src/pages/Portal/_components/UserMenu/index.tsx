import React from "react";

import { Badge } from "antd";
import {
  ControlOutlined,
  LaptopOutlined,
  LogoutOutlined,
  SafetyOutlined,
  SettingOutlined,
} from "@ant-design/icons";

import { inNativeShell } from "@/utils/clientAuth";
import type { PortalUserInfo } from "../../_context/portalUser";
import type { SettingsTab } from "../../_utils/portalNav";
import styles from "./index.module.scss";

interface UserMenuProps {
  user: PortalUserInfo;
  pendingCount: number;
  onOpenSettings: (tab: SettingsTab) => void;
  onLogout: () => void;
  onClose: () => void;
}

/**
 * 侧栏底部账户菜单的内容（对齐 claude.ai 侧栏底部的账户菜单）。
 *
 * 「设置 / 设备管理 / 安全防护」在档 B 之后都是**真实地址**，点它们走 navigate，
 * 而不再是打开一个弹窗 —— 所以本组件只发出「去哪一页」，跳转由壳负责。
 */
const UserMenu: React.FC<UserMenuProps> = (props) => {
  const { user, pendingCount, onOpenSettings, onLogout, onClose } = props;
  const nickname = user.nickname ?? user.username ?? "";

  return (
    <div className={styles.userMenu}>
      <div className={styles.userMenuEmail}>{user.username}</div>
      <div className={styles.userMenuAccount}>
        <span className={styles.userMenuAvatar}>{nickname.slice(0, 1) || "U"}</span>
        <span className={styles.userMenuName}>{nickname}</span>
      </div>
      <div className={styles.userMenuDivider} />
      <div className={styles.userMenuItem} onClick={() => onOpenSettings("account")}>
        <SettingOutlined />
        <span>设置</span>
      </div>
      <div className={styles.userMenuItem} onClick={() => onOpenSettings("devices")}>
        <LaptopOutlined />
        <span>设备管理</span>
        {pendingCount ? (
          <Badge count={pendingCount} size="small" className={styles.userMenuBadge} />
        ) : null}
      </div>
      <div className={styles.userMenuItem} onClick={() => onOpenSettings("security")}>
        <SafetyOutlined />
        <span>安全防护</span>
      </div>
      {/* 后台管理只在浏览器里显示：客户端 / 移动端原生壳内隐藏（那里开新标签打不开后管） */}
      {user.isSuper && !inNativeShell() ? (
        <div
          className={styles.userMenuItem}
          onClick={() => {
            window.open("/admin", "_blank");
            onClose();
          }}
        >
          <ControlOutlined />
          <span>后台管理</span>
        </div>
      ) : null}
      <div className={styles.userMenuDivider} />
      <div className={styles.userMenuItem} onClick={onLogout}>
        <LogoutOutlined />
        <span>退出登录</span>
      </div>
    </div>
  );
};

export default UserMenu;
