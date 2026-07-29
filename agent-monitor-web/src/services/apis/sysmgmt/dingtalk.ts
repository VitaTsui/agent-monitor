import { get, post } from "@/services/Axios";

/** 全局钉钉企业应用配置（后管统一配置，全体用户共用一个机器人） */
export interface DingtalkAppAdminInfo {
  /** 是否已设置 AppSecret（密钥不回显） */
  hasSecret: boolean;
  /** AppKey / ClientID（可见） */
  appKey: string;
  /** 是否走 Stream 长连接（填了 AppKey 即为 true） */
  stream: boolean;
  /** HTTP 回调模式的回调地址（Stream 模式为 null） */
  callbackUrl: string | null;
}

export const getDingtalkAppAdmin = () => {
  return get<DingtalkAppAdminInfo>("/sys/dingtalk/app");
};

/** 设置全局钉钉企业应用。appSecret 留空=沿用已存；appKey 空=清空回 HTTP 回调模式 */
export const setDingtalkAppAdmin = (data: {
  appSecret?: string;
  appKey?: string;
}) => {
  return post<{ callbackUrl: string | null; stream: boolean }>(
    "/sys/dingtalk/app",
    data
  );
};

/** 全局钉钉群机器人（webhook 推送）配置 */
export interface DingtalkRobotAdminInfo {
  webhook: string;
  hasSecret: boolean;
  waiting: boolean;
  finished: boolean;
  newSession: boolean;
  device: boolean;
}

export const getDingtalkRobotAdmin = () => {
  return get<DingtalkRobotAdminInfo>("/sys/dingtalk/robot");
};

export const setDingtalkRobotAdmin = (data: {
  webhook: string;
  secret?: string;
  waiting: boolean;
  finished: boolean;
  newSession: boolean;
  device: boolean;
}) => {
  return post<boolean>("/sys/dingtalk/robot", data);
};

export const testDingtalkRobotAdmin = () => {
  return post<boolean>("/sys/dingtalk/robot/test", {});
};

/** 全局企业微信自建应用配置 */
export interface WecomAppAdminInfo {
  corpId: string;
  token: string;
  hasAesKey: boolean;
  callbackUrl: string | null;
}

export const getWecomAppAdmin = () => {
  return get<WecomAppAdminInfo>("/sys/wecom/app");
};

/** aesKey 留空=沿用已存 */
export const setWecomAppAdmin = (data: {
  corpId: string;
  token?: string;
  aesKey?: string;
}) => {
  return post<{ callbackUrl: string | null }>("/sys/wecom/app", data);
};
