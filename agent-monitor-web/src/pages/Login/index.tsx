// antd Form 仅作表单容器（hsu-ui Form 只含 Modal/Drawer/Import/useForm，无普通容器）
import { Divider, Form, message, Segmented } from "antd";
import {
  AppleFilled,
  CodeOutlined,
  DingtalkOutlined,
  GoogleOutlined,
  LockOutlined,
  SmileOutlined,
  UserOutlined,
} from "@ant-design/icons";

import { Button, FormItem } from "@hsu-react/ui";
import LoginStore from "./LoginStore";
import type { OAuthProvider } from "@/services/apis/login";
import React, { useEffect, useState } from "react";
import { observer } from "mobx-react-lite";
import styles from "./index.module.scss";
import { useDebounceEffect } from "ahooks";
import { useNavigate } from "react-router-dom";

const DEFAULT_PATH = process.env.DEFAULT_PATH ?? "/portal";
const DINGTALK_STATE_KEY = "dingtalk_oauth_state";
const OAUTH_STATE_KEY = "oauth_state";
const OAUTH_PROVIDER_KEY = "oauth_provider";
/** 第三方往返后 query 里不再有 redirect，发起前存下目标页 */
const OAUTH_REDIRECT_KEY = "oauth_redirect";

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
  // undefined = 探测中（按钮显示加载态，避免先闪一下禁用灰）
  const [googleEnabled, setGoogleEnabled] = useState<boolean | undefined>();
  const [appleEnabled, setAppleEnabled] = useState<boolean | undefined>();
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
    checkOAuthProviders,
    gotoOAuth,
    oauthLogin,
  } = LoginStore;

  useDebounceEffect(() => {
    getCryptoKey();

    checkIsNeedLoginCaptcha();
  }, [checkIsNeedLoginCaptcha, getCryptoKey]);

  // 探测各第三方渠道是否开启（未配置则按钮禁用）+ 处理各类 OAuth 回调
  useEffect(() => {
    checkDingtalkEnabled().then(setDingtalkEnabled);
    // 一次请求拿到所有渠道开关
    checkOAuthProviders().then((p) => {
      setGoogleEnabled(p.google);
      setAppleEnabled(p.apple);
    });

    const params = new URLSearchParams(window.location.search);

    // 钉钉回调
    const authCode = params.get("authCode");
    if (authCode) {
      const state = params.get("state") ?? undefined;
      const saved = sessionStorage.getItem(DINGTALK_STATE_KEY);
      const back = sessionStorage.getItem(OAUTH_REDIRECT_KEY) || redirectTo;
      sessionStorage.removeItem(DINGTALK_STATE_KEY);
      sessionStorage.removeItem(OAUTH_REDIRECT_KEY);
      window.history.replaceState({}, "", window.location.pathname);
      // 严格校验：本地必须有已发起记录，且回调 state 必须与之完全一致
      if (!saved || !state || saved !== state) {
        message.error("钉钉登录校验失败，请重新发起登录");
        return;
      }
      setSubmitting(true);
      dingtalkLogin(authCode, state, () => {
        message.success("登录成功");
        navigate(back);
      })
        .catch(() => message.error("登录失败，请检查网络后重试"))
        .finally(() => setSubmitting(false));
      return;
    }

    // Google / Apple 回调（?code=&state=）
    const code = params.get("code");
    if (code) {
      const state = params.get("state") ?? undefined;
      const provider = sessionStorage.getItem(OAUTH_PROVIDER_KEY) as
        | OAuthProvider
        | null;
      const saved = sessionStorage.getItem(OAUTH_STATE_KEY);
      // OAuth 往返后 query 里已无 redirect，用发起前存的目标回跳
      const back = sessionStorage.getItem(OAUTH_REDIRECT_KEY) || redirectTo;
      sessionStorage.removeItem(OAUTH_STATE_KEY);
      sessionStorage.removeItem(OAUTH_PROVIDER_KEY);
      sessionStorage.removeItem(OAUTH_REDIRECT_KEY);
      window.history.replaceState({}, "", window.location.pathname);
      // 严格校验：缺 provider / 缺本地 state / state 不符，一律拒绝并提示
      if (!provider || !saved || !state || saved !== state) {
        message.error("登录会话已失效或校验失败，请重新发起登录");
        return;
      }
      setSubmitting(true);
      oauthLogin(provider, code, state, () => {
        message.success("登录成功");
        navigate(back);
      })
        .catch(() => message.error("登录失败，请检查网络后重试"))
        .finally(() => setSubmitting(false));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // OAuth state 是防登录劫持的一次性凭据，必须不可预测：
  // Math.random 不是密码学随机源，这里用 crypto。
  const genState = () => window.crypto.randomUUID();

  const onDingtalk = () => {
    const state = genState();
    sessionStorage.setItem(DINGTALK_STATE_KEY, state);
    sessionStorage.setItem(OAUTH_REDIRECT_KEY, redirectTo);
    gotoDingtalk(state)
      .then((ok) => {
        if (!ok) message.error("钉钉登录暂不可用");
      })
      .catch(() => message.error("钉钉登录暂不可用"));
  };

  const onOAuth = (provider: OAuthProvider, enabled: boolean) => {
    if (!enabled) {
      message.info(
        provider === "google"
          ? "管理员尚未配置 Google 登录"
          : "管理员尚未配置 Apple 登录",
      );
      return;
    }
    const state = genState();
    sessionStorage.setItem(OAUTH_STATE_KEY, state);
    sessionStorage.setItem(OAUTH_PROVIDER_KEY, provider);
    sessionStorage.setItem(OAUTH_REDIRECT_KEY, redirectTo);
    gotoOAuth(provider, state)
      .then((ok) => {
        if (!ok) message.error("第三方登录暂不可用");
      })
      .catch(() => message.error("第三方登录暂不可用"));
  };

  const switchMode = (next: Mode) => {
    if (next === mode) return;
    setMode(next);
    form.resetFields();
  };

  const onLogin = () => {
    // validateFields 校验不过就是 reject（antd 约定），静默吞掉即可；
    // login 的网络异常则要给出可见提示。
    form
      .validateFields()
      .then((values) => {
        setSubmitting(true);
        login(values, () => {
          navigate(redirectTo);
        })
          .catch(() => message.error("登录失败，请检查网络后重试"))
          .finally(() => setSubmitting(false));
      })
      .catch(() => void 0);
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
      )
        .catch(() => message.error("注册失败，请检查网络后重试"))
        .finally(() => setSubmitting(false));
    })
      .catch(() => void 0);
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
      <div className={styles.card}>
        <div className={styles.brand}>
          <div className={styles.logo}>
            <CodeOutlined />
          </div>
          <div className={styles.brandName}>终端任务监控</div>
          <div className={styles.brandSub}>
            监控多台电脑终端里 AI 编码代理正在执行的任务
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
            inputHeight={48}
            componentProps={{
              placeholder: "用户名",
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
              inputHeight={48}
              componentProps={{
                placeholder: "昵称（选填）",
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
            inputHeight={48}
            componentProps={{
              placeholder: mode === "register" ? "设置密码（6~30 位）" : "密码",
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
              inputHeight={48}
              componentProps={{
                placeholder: "确认密码",
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
            inputHeight={48}
            componentProps={{
              placeholder: "验证码",
              suffix: (
                <img
                  className={styles.captcha}
                  src={captchaImg}
                  alt="验证码，点击刷新"
                  title="点击刷新验证码"
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

        {/* 全部渠道都未接入时，连同分隔线一起不渲染 */}
        {(googleEnabled !== false || appleEnabled !== false || dingtalkEnabled) && (
        <Divider className={styles.orDivider} plain>
          或使用第三方账号{mode === "register" ? "注册" : "登录"}
        </Divider>
        )}

        {/* 未接入的渠道整个不渲染（探测中先渲染加载态占位，避免布局跳动） */}
        <div className={styles.socials}>
          {googleEnabled !== false && (
            <Button
              className={styles.socialBtn}
              icon={<GoogleOutlined />}
              loading={googleEnabled === undefined}
              onClick={() => onOAuth("google", !!googleEnabled)}
            >
              Google
            </Button>
          )}
          {appleEnabled !== false && (
            <Button
              className={styles.socialBtn}
              icon={<AppleFilled />}
              loading={appleEnabled === undefined}
              onClick={() => onOAuth("apple", !!appleEnabled)}
            >
              Apple
            </Button>
          )}
          {dingtalkEnabled && (
            <Button
              className={styles.socialBtn}
              icon={<DingtalkOutlined />}
              onClick={onDingtalk}
            >
              钉钉
            </Button>
          )}
        </div>

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
      </div>
    </div>
  );
});

export default Login;
