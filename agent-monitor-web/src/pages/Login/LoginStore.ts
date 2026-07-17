import { setToken, setUserInfo } from "@/utils/auth";

import {
  getCryptoKey,
  login,
  LoginData,
  register,
  RegisterData,
  isNeedLoginCaptcha,
  logout,
  getDingtalkUrl,
  dingtalkLogin,
  getOAuthUrl,
  getOAuthProviders,
  oauthLogin,
  OAuthProvider,
} from "@/services/apis/login";
import crypto from "@/utils/crypto";
import { getUUID } from "@/utils";
import { makeAutoObservable } from "mobx";

import RouterService from "@/router/RouterService";
import { notification } from "antd";
import Cookies from "js-cookie";

const dev = process.env.NODE_ENV === "development";
const apiBase = process.env.API_BASE;

class LoginStore {
  get captchaImg() {
    return this._captchaImg;
  }
  private _captchaImg: string = "";

  get isNeedLoginCaptcha() {
    return this._isNeedLoginCaptcha;
  }
  private _isNeedLoginCaptcha: boolean = false;

  private _codeKey: string = "";

  private _cryptoKey: string = "";

  constructor() {
    makeAutoObservable(this);
  }

  public getCryptoKey = async () => {
    const res = await getCryptoKey();
    if (res.code === 0) {
      this._cryptoKey = res.data;
    }
  };

  /**
   * 确保会话密钥已就绪。页面加载时的 getCryptoKey 若因网络抖动失败，
   * _cryptoKey 会一直是空串——不补救的话，登录会带着空密钥走进加密流程，
   * 抛出的会是一段跟「网络」毫无关系的加密错误，用户完全无从排查。
   */
  private ensureCryptoKey = async () => {
    if (!this._cryptoKey) {
      await this.getCryptoKey().catch(() => void 0);
    }
    if (!this._cryptoKey) {
      notification.error({ message: "安全连接未就绪，请检查网络后重试" });
      return false;
    }
    return true;
  };

  // 检查是否需要登录验证码
  public checkIsNeedLoginCaptcha = async () => {
    try {
      const res = await isNeedLoginCaptcha();
      if (res.code === 0) {
        this._isNeedLoginCaptcha = res.data;
        if (this._isNeedLoginCaptcha) {
          this.getCaptchaImg();
        }
      }
    } catch {
      void 0;
    }
  };

  public login = async (
    data: Omit<LoginData, "cryptoKey" | "codeKey">,
    fn?: () => void,
  ) => {
    if (!(await this.ensureCryptoKey())) {
      return;
    }
    const key = await crypto.decrypt(this._cryptoKey);
    const username = await crypto.encodeRSA(
      await crypto.encrypt(data.username, key),
    );
    const password = await crypto.encodeRSA(
      await crypto.encrypt(data.password, key),
    );

    const form: LoginData = {
      ...data,
      cryptoKey: this._cryptoKey,
      username,
      password,
      codeKey: this._codeKey,
    };

    const res = await login(form);
    if (res.code === 0) {
      const data = res.data;

      setToken(data.token);
      setUserInfo(data.userInfo);
      Cookies.set("tenant-id", "0");

      await RouterService.getMenuList(true);

      await RouterService.getPermissions(true);

      fn?.();
    } else {
      notification.error({
        message: res?.msg ?? "失败",
      });

      this.checkIsNeedLoginCaptcha();
    }
  };

  public register = async (
    data: { username: string; password: string; nickname?: string } & Pick<
      LoginData,
      "codeVal"
    >,
    fn?: () => void,
  ) => {
    if (!(await this.ensureCryptoKey())) {
      return;
    }
    const key = await crypto.decrypt(this._cryptoKey);
    const username = await crypto.encodeRSA(
      await crypto.encrypt(data.username, key),
    );
    const password = await crypto.encodeRSA(
      await crypto.encrypt(data.password, key),
    );

    const form: RegisterData = {
      cryptoKey: this._cryptoKey,
      username,
      password,
      nickname: data.nickname,
      codeKey: this._codeKey,
      codeVal: data.codeVal,
    };

    const res = await register(form);
    if (res.code === 0) {
      const resData = res.data;

      setToken(resData.token);
      setUserInfo(resData.userInfo);
      Cookies.set("tenant-id", "0");

      await RouterService.getMenuList(true);
      await RouterService.getPermissions(true);

      fn?.();
    } else {
      notification.error({
        message: res?.msg ?? "注册失败",
      });

      this.checkIsNeedLoginCaptcha();
    }
  };

  // 取钉钉扫码授权地址并跳转；未开启时返回 false 由页面隐藏入口
  public gotoDingtalk = async (state: string): Promise<boolean> => {
    const res = await getDingtalkUrl(state);
    if (res.code === 0 && res.data?.enabled && res.data.url) {
      window.location.href = res.data.url;
      return true;
    }
    return false;
  };

  // 检测钉钉是否开启（控制登录页入口显隐）
  public checkDingtalkEnabled = async (): Promise<boolean> => {
    try {
      const res = await getDingtalkUrl("");
      return res.code === 0 && !!res.data?.enabled;
    } catch {
      return false;
    }
  };

  // 钉钉回调授权码换登录态
  public dingtalkLogin = async (
    authCode: string,
    state: string | undefined,
    fn?: () => void,
  ) => {
    const res = await dingtalkLogin({ authCode, state });
    if (res.code === 0) {
      const data = res.data;
      setToken(data.token);
      setUserInfo(data.userInfo);
      Cookies.set("tenant-id", "0");

      await RouterService.getMenuList(true);
      await RouterService.getPermissions(true);

      fn?.();
    } else {
      notification.error({
        message: res?.msg ?? "钉钉登录失败",
      });
    }
  };

  // ---------- 第三方 OAuth（Google / Apple） ----------

  // 一次性探测各渠道开关（首屏只打一个请求）
  public checkOAuthProviders = async (): Promise<
    Record<OAuthProvider, boolean>
  > => {
    try {
      const res = await getOAuthProviders();
      if (res.code === 0 && res.data) {
        return { google: !!res.data.google, apple: !!res.data.apple };
      }
    } catch {
      // 探测失败按未配置处理
    }
    return { google: false, apple: false };
  };

  // 跳转到第三方授权页；未配置返回 false，由页面提示
  public gotoOAuth = async (
    provider: OAuthProvider,
    state: string,
  ): Promise<boolean> => {
    const res = await getOAuthUrl(provider, state);
    if (res.code === 0 && res.data?.enabled && res.data.url) {
      window.location.href = res.data.url;
      return true;
    }
    return false;
  };

  // 第三方回调授权码换登录态（后端不存在则自动注册）
  public oauthLogin = async (
    provider: OAuthProvider,
    code: string,
    state: string | undefined,
    fn?: () => void,
  ) => {
    const res = await oauthLogin(provider, { code, state });
    if (res.code === 0) {
      const data = res.data;
      setToken(data.token);
      setUserInfo(data.userInfo);
      Cookies.set("tenant-id", "0");

      await RouterService.getMenuList(true);
      await RouterService.getPermissions(true);

      fn?.();
    } else {
      notification.error({
        message: res?.msg ?? "第三方登录失败",
      });
    }
  };

  public getCaptchaImg = () => {
    this._codeKey = getUUID();
    this._captchaImg = `${dev ? (apiBase ?? "/api") : ""}/auth/kaptcha/generate/${
      this._codeKey
    }`;
    this._isNeedLoginCaptcha = true;
  };

  public logout = (fn?: () => void) => {
    logout()
      .then((res) => {
        if (res.code === 0) {
          fn?.();
        } else {
          notification.error({
            message: res.msg,
          });
        }
      })
      // 网络异常时也必须走 fn（清 cookie + 跳登录页）：服务端登录态最终会过期，
      // 把用户困在「点了退出却毫无反应」的页面上更糟。改密码后的强制重登也走这条路。
      .catch(() => {
        notification.warning({
          message: "退出登录请求失败，已在本地清除登录状态",
        });
        fn?.();
      });
  };
}

export default new LoginStore();
