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
