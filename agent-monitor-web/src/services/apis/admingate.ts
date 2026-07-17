/**************************************
 * Module Name : 后管访问令牌校验（跨页共享）
 **************************************/

import { post } from "@/services/Axios";

// 校验部署时生成的后管访问令牌（需已登录且为管理员）
export const verifyAdminToken = async (token: string) => {
  return await post<boolean>("/sys/admin/verify", { token });
};
