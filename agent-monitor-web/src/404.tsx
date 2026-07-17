import { Result } from "antd";
import { Button } from "@hsu-react/ui";

import React from "react";
import { useNavigate } from "react-router";

const NoFoundPage: React.FC = () => {
  const navigate = useNavigate();

  return (
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
  );
};

export default NoFoundPage;
