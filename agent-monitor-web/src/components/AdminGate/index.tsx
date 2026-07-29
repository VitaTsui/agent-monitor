import React from "react";

interface AdminGateProps {
  children: React.ReactNode;
}

/**
 * 后管准入：已由「已登录 + 超级管理员」把关（前台仅超级管理员显示「后台管理」入口，
 * 后端所有 /sys/* 也校验超级管理员身份），不再额外要求部署令牌，直接放行。
 */
const AdminGate: React.FC<AdminGateProps> = ({ children }) => {
  return <>{children}</>;
};

export default AdminGate;
