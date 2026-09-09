import { ConfigProvider, Result } from "antd";
import { Button } from "@hsu-react/ui";

import React from "react";
import { useNavigate } from "react-router";

// 兜底路由挂在 App/Theme 之外，拿不到全局 ConfigProvider——
// 自带一份品牌主色，避免退化成 antd 默认蓝。
const NoFoundPage: React.FC = () => {
  const navigate = useNavigate();

  return (
    <ConfigProvider
      theme={{ token: { colorPrimary: "#18181b", colorLink: "#18181b" } }}
    >
      <div
        style={{
          minHeight: "100vh",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          background: "linear-gradient(180deg, #f4fafb 0%, #eaf4f6 100%)",
        }}
      >
        <Result
          status="404"
          title="404"
          subTitle="抱歉，你访问的页面不存在。"
          extra={
            <Button type="primary" onClick={() => navigate("/")}>
              返回首页
            </Button>
          }
        ></Result>
      </div>
    </ConfigProvider>
  );
};

export default NoFoundPage;
