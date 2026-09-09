import React from "react";

import { Button } from "@hsu-react/ui";
import { LogoutOutlined } from "@ant-design/icons";

import { removeToken } from "@/utils/auth";
import { usePortalUser } from "../../../_context/portalUser";
import views from "../../views.module.scss";
import st from "../settings.module.scss";

/** 账户（`/portal/settings/account`）。 */
const AccountView: React.FC = () => {
  const user = usePortalUser();

  const onLogout = () => {
    removeToken();
    window.location.href = "/login?redirect=%2Fportal";
  };

  return (
    <div className={views.pageFixed}>
      <div>
        <div className={`${views.fixedHead} ${st.paneHead}`}>
          <div className={views.headRow}>
            <span className={views.headTitle}>账户</span>
          </div>
        </div>
        <div className={views.fixedBody}>
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
        </div>
      </div>
    </div>
  );
};

export default AccountView;
