import React from "react";

import { Button } from "@hsu-react/ui";
import { LogoutOutlined } from "@ant-design/icons";

import { removeToken } from "@/utils/auth";
import { usePortalUser } from "../../../../_context/portalUser";
import st from "../../settings.module.scss";

/** 账户分栏。 */
const AccountPane: React.FC = () => {
  const user = usePortalUser();

  const onLogout = () => {
    removeToken();
    window.location.href = "/login?redirect=%2Fportal";
  };

  return (
    <>
      <div className={st.paneTitle}>账户</div>
      <div className={st.account}>
        <div className={st.avatar}>
          {(user.nickname ?? user.username ?? "U").slice(0, 1)}
        </div>
        <div>
          <div className={st.accName}>{user.nickname ?? user.username}</div>
          <div className={st.accMeta}>
            用户名 {user.username}
            {user.isSuper ? " · 超级管理员" : ""}
          </div>
        </div>
      </div>
      <Button icon={<LogoutOutlined />} danger onClick={onLogout}>
        退出登录
      </Button>
    </>
  );
};

export default AccountPane;
