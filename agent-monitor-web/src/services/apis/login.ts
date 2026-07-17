import { get, post } from "../Axios";

// 登录
export type LoginData = {
  username: string;
  password: string;
  cryptoKey: string;
  codeKey: string;
  codeVal: string;
};
export interface UserInfo {}
export interface LoginResData {
  userInfo: UserInfo;
  token: string;
}
export const login = async (data: LoginData) => {
  return await post<LoginResData>("/auth/access/login", data);
};

// 注册
export type RegisterData = {
  username: string;
  password: string;
  cryptoKey: string;
  nickname?: string;
  codeKey?: string;
  codeVal?: string;
};
export const register = async (data: RegisterData) => {
  return await post<LoginResData>("/auth/access/register", data);
};

// 加密 Key
export const getCryptoKey = async () => {
  return await get<string>("/auth/access/getCryptoKey");
};

// 退出
export const logout = async () => {
  return await get("/auth/access/logout");
};

// 是否需要验证码
export const isNeedLoginCaptcha = async () => {
  return await get<boolean>("/auth/access/isNeedLoginCaptcha");
};

// 钉钉扫码授权地址
export type DingtalkUrlRes = { enabled: boolean; url: string };
export const getDingtalkUrl = async (state: string) => {
  return await get<DingtalkUrlRes>("/auth/access/dingtalk/url", {
    params: { state },
    // 登录页未登录态探测：401 时静默隐藏入口，不触发跳登录
    skipAuthRedirect: true,
  });
};

// 钉钉扫码登录（用回调授权码换登录态）
export const dingtalkLogin = async (data: {
  authCode: string;
  state?: string;
}) => {
  return await post<LoginResData>("/auth/access/dingtalk", data);
};

// ---------- 第三方 OAuth（Google / Apple） ----------

export type OAuthProvider = "google" | "apple";
export type OAuthUrlRes = { enabled: boolean; url: string };

// 一次性取各第三方渠道开关（首屏只打一个请求）
export type OAuthProvidersRes = Record<OAuthProvider, boolean>;
export const getOAuthProviders = async () => {
  return await get<OAuthProvidersRes>("/auth/access/oauth/providers", {
    skipAuthRedirect: true,
  });
};

// 取第三方登录授权地址（enabled=false 表示后端未配置该渠道）
export const getOAuthUrl = async (provider: OAuthProvider, state: string) => {
  return await get<OAuthUrlRes>(`/auth/access/oauth/${provider}/url`, {
    params: { state },
    skipAuthRedirect: true,
  });
};

// 第三方回调授权码换登录态（不存在则自动注册）
export const oauthLogin = async (
  provider: OAuthProvider,
  data: { code: string; state?: string },
) => {
  return await post<LoginResData>(`/auth/access/oauth/${provider}/login`, data);
};
