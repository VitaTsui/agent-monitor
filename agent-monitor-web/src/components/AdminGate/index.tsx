import React, { useState } from "react";

import { Button, Input } from "@hsu-react/ui";
import { message } from "antd";
import { LockOutlined } from "@ant-design/icons";

import { verifyAdminToken } from "@/services/apis/admingate";
import { getAdminToken, setAdminToken } from "@/utils/auth";
import styles from "./index.module.scss";

interface AdminGateProps {
  children: React.ReactNode;
}

/**
 * 后管访问锁：部署时生成的令牌（X-Admin-Token）校验通过后才放行后管页面。
 * 令牌存 sessionStorage，关闭标签页失效；后端所有 /sys/* 管理接口同样校验该头。
 */
const AdminGate: React.FC<AdminGateProps> = (props) => {
  const { children } = props;
  const [unlocked] = useState(() => !!getAdminToken());
  const [token, setToken] = useState("");
  const [verifying, setVerifying] = useState(false);

  const unlock = () => {
    const value = token.trim();
    if (!value) {
      return;
    }
    setVerifying(true);
    verifyAdminToken(value)
      .then((res) => {
        if (res.code === 0) {
          setAdminToken(value);
          // 整页刷新：让动态菜单/权限带上令牌重新拉取
          window.location.reload();
        } else {
          message.error(res.msg ?? "令牌不正确");
        }
      })
      .catch(() => {
        message.error("校验请求失败，请稍后重试");
      })
      .finally(() => setVerifying(false));
  };

  if (unlocked) {
    return <>{children}</>;
  }

  return (
    <div className={styles.AdminGate}>
      <div className={styles.card}>
        <div className={styles.lockIcon}>
          <LockOutlined />
        </div>
        <div className={styles.title}>后台管理已锁定</div>
        <div className={styles.desc}>
          请输入部署时生成的后管访问令牌（服务端日志或
          <code>~/.agent-monitor/admin-token</code>）
        </div>
        <Input.Password
          className={styles.input}
          placeholder="后管访问令牌"
          value={token}
          onChange={(value) => setToken(value)}
          onPressEnter={unlock}
        />
        <Button
          type="primary"
          block
          loading={verifying}
          disabled={!token.trim()}
          onClick={unlock}
        >
          解 锁
        </Button>
      </div>
    </div>
  );
};

export default AdminGate;
