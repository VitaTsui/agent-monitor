import { Result } from "antd";
import { Button } from "@hsu-react/ui";

import React from "react";
import { useNavigate } from "react-router";

// 主题不在这里配：外观控制器与 antd 主题都在应用根上（src/index.tsx 的 Layout.Theme
// ＋ router/Routes.tsx 的 ConfigProvider），本页在它们里面。
// 原来这里自带一份 `colorPrimary: "#18181b"` ＋ 一片写死的青灰渐变底 —— 前者在暗色下
// 与令牌那侧的主色对不上，后者根本不跟主题走，暗色时整页是一块亮色。
const NoFoundPage: React.FC = () => {
  const navigate = useNavigate();

  return (
    <div
      style={{
        minHeight: "100vh",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        background: "var(--background)",
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
  );
};

export default NoFoundPage;
