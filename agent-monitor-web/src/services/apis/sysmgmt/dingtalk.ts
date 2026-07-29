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
