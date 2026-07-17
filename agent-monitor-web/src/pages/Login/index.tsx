// antd Form 仅作表单容器（hsu-ui Form 只含 Modal/Drawer/Import/useForm，无普通容器）
import { Divider, Form, message, Segmented } from "antd";
import {
  DingtalkOutlined,
  LockOutlined,
  SmileOutlined,
  UserOutlined,
} from "@ant-design/icons";

import { Button, FormItem } from "@hsu-react/ui";
import LoginStore from "./LoginStore";
import React, { useEffect, useState } from "react";
import { observer } from "mobx-react-lite";
import styles from "./index.module.scss";
import { useDebounceEffect } from "ahooks";
import { useNavigate } from "react-router-dom";

const DEFAULT_PATH = process.env.DEFAULT_PATH ?? "/portal";
const DINGTALK_STATE_KEY = "dingtalk_oauth_state";

type Mode = "login" | "register";

const Login: React.FC = observer(() => {
  const navigate = useNavigate();
  // 支持登录后跳回来源页（如前台 /portal）
  const redirectTo = (() => {
    const r = new URLSearchParams(window.location.search).get("redirect");
    return r && r.startsWith("/") ? r : DEFAULT_PATH;
  })();
  const [form] = Form.useForm();
  const [mode, setMode] = useState<Mode>("login");
  const [submitting, setSubmitting] = useState(false);
  const [dingtalkEnabled, setDingtalkEnabled] = useState(false);
  const {
    login,
    register,
    captchaImg,
    getCaptchaImg,
    checkIsNeedLoginCaptcha,
    isNeedLoginCaptcha,
    getCryptoKey,
    gotoDingtalk,
    checkDingtalkEnabled,
    dingtalkLogin,
  } = LoginStore;

  useDebounceEffect(() => {
    getCryptoKey();

    checkIsNeedLoginCaptcha();
  }, [checkIsNeedLoginCaptcha, getCryptoKey]);

  // 钉钉：探测开关（未配置则隐藏入口）+ 处理扫码回调
  // 探测请求带 skipAuthRedirect，401 时静默当作未开启，不触发跳登录
  useEffect(() => {
    checkDingtalkEnabled().then(setDingtalkEnabled);

    const params = new URLSearchParams(window.location.search);
    const authCode = params.get("authCode");
    if (!authCode) return;
    const state = params.get("state") ?? undefined;
    const saved = sessionStorage.getItem(DINGTALK_STATE_KEY);
    // 清掉地址栏的回调参数，避免刷新重复触发
    window.history.replaceState({}, "", window.location.pathname);
    if (saved && state && saved !== state) {
      message.error("钉钉登录校验失败，请重试");
      return;
    }
    sessionStorage.removeItem(DINGTALK_STATE_KEY);
    setSubmitting(true);
    dingtalkLogin(authCode, state, () => {
      message.success("登录成功");
      navigate(DEFAULT_PATH);
    }).finally(() => setSubmitting(false));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const onDingtalk = () => {
    const state = `${Date.now()}_${Math.random().toString(36).slice(2)}`;
    sessionStorage.setItem(DINGTALK_STATE_KEY, state);
    gotoDingtalk(state).then((ok) => {
      if (!ok) message.error("钉钉登录暂不可用");
    });
  };

  const switchMode = (next: Mode) => {
    if (next === mode) return;
    setMode(next);
    form.resetFields();
  };

  const onLogin = () => {
    form.validateFields().then((values) => {
      setSubmitting(true);
      login(values, () => {
        navigate(redirectTo);
      }).finally(() => setSubmitting(false));
    });
  };

  const onRegister = () => {
    form.validateFields().then((values) => {
      if (values.password !== values.confirmPassword) {
        message.error("两次输入的密码不一致");
        return;
      }
      setSubmitting(true);
      register(
        {
          username: values.username,
          password: values.password,
          nickname: values.nickname,
          codeVal: values.codeVal,
        },
        () => {
          message.success("注册成功，已自动登录");
          navigate(redirectTo);
        },
      ).finally(() => setSubmitting(false));
    });
  };

  const submit = mode === "login" ? onLogin : onRegister;

  const onEnter = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      submit();
    }
  };

  return (
    <div className={styles.login}>
      <div className={styles.brand}>
        <div className={styles.brandInner}>
          <div className={styles.brandLogo}>{Config.title?.[0] ?? "AI"}</div>
          <div className={styles.brandTitle}>{Config.title ?? "AI 平台"}</div>
          <div className={styles.brandSlogan}>
            {Config.description ||
              "监控多台电脑终端里 AI 编码代理正在执行的任务"}
          </div>
        </div>
      </div>

      <div className={styles.panel}>
        <div className={styles.card}>
          <div className={styles.cardHead}>
            <div className={styles.cardTitle}>
              {mode === "login" ? "欢迎回来" : "创建账号"}
            </div>
            <div className={styles.cardSub}>
              {mode === "login"
                ? "登录以继续使用平台"
                : "几秒钟即可完成注册"}
            </div>
          </div>

          <Segmented
            block
            className={styles.tabs}
            value={mode}
            onChange={(v) => switchMode(v as Mode)}
            options={[
              { label: "登录", value: "login" },
              { label: "注册", value: "register" },
            ]}
          />

          <Form form={form} className={styles.form}>
            <FormItem
              type="INPUT"
              name="username"
              className={styles.formitem}
              required
              requiredMsg="请输入用户名"
              inputHeight={52}
              componentProps={{
                placeholder: "请输入用户名",
                prefix: <UserOutlined />,
                className: styles.input,
                onKeyDown: onEnter,
              }}
            />

            {mode === "register" && (
              <FormItem
                type="INPUT"
                name="nickname"
                className={styles.formitem}
                inputHeight={52}
                componentProps={{
                  placeholder: "请输入昵称（选填）",
                  prefix: <SmileOutlined />,
                  className: styles.input,
                  onKeyDown: onEnter,
                }}
              />
            )}

            <FormItem
              type="PASSWORD"
              name="password"
              className={styles.formitem}
              required
              requiredMsg="请输入密码"
              inputHeight={52}
              componentProps={{
                placeholder: mode === "register" ? "请设置密码（6~30 位）" : "请输入密码",
                prefix: <LockOutlined />,
                className: styles.input,
                onKeyDown: onEnter,
              }}
            />

            {mode === "register" && (
              <FormItem
                type="PASSWORD"
                name="confirmPassword"
                className={styles.formitem}
                required
                requiredMsg="请再次输入密码"
                inputHeight={52}
                componentProps={{
                  placeholder: "请再次输入密码",
                  prefix: <LockOutlined />,
                  className: styles.input,
                  onKeyDown: onEnter,
                }}
              />
            )}

            <FormItem
              type="INPUT"
              name="codeVal"
              className={styles.formitem}
              required
              requiredMsg="请输入验证码"
              inputHeight={52}
              componentProps={{
                placeholder: "请输入验证码",
                suffix: (
                  <img
                    className={styles.captcha}
                    src={captchaImg}
                    onClick={getCaptchaImg}
                  />
                ),
                className: styles.input,
                onKeyDown: onEnter,
              }}
              visible={isNeedLoginCaptcha}
            />
          </Form>

          <Button
            type="primary"
            className={styles.button}
            loading={submitting}
            onClick={submit}
          >
            {mode === "login" ? "登 录" : "注 册"}
          </Button>

          <div className={styles.switchTip}>
            {mode === "login" ? (
              <>
                还没有账号？
                <a onClick={() => switchMode("register")}>立即注册</a>
              </>
            ) : (
              <>
                已有账号？
                <a onClick={() => switchMode("login")}>返回登录</a>
              </>
            )}
          </div>

          {dingtalkEnabled && (
            <>
              <Divider className={styles.otherDivider} plain>
                其他登录方式
              </Divider>
              <div className={styles.otherLogin}>
                <button
                  type="button"
                  className={styles.dingtalkBtn}
                  onClick={onDingtalk}
                  title="钉钉扫码登录"
                >
                  <DingtalkOutlined />
                </button>
              </div>
            </>
          )}
        </div>
      </div>
    </div>
  );
});

export default Login;
