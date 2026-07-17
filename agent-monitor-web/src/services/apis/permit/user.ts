/**************************************
 * Module Name : 后管-用户管理
 **************************************/

import { get, post } from "@/services/Axios";

import { ListRes } from "@/services/ResType";

// 页面搜索态
export interface UserSearchData extends Record<string, unknown> {
  username?: string;
}

// 请求参数
interface UserSearch extends Record<string, unknown> {
  query: string;
}

// 实体（hub 用户注册表）
interface IUserData {
  id: string | number;
  username: string;
  nickname: string;
  isSuper: boolean;
  roleDsr: string;
  deviceCount: number;
}
export type UserData = Partial<IUserData>;

// 列表
export const getUserList = async (params: UserSearch) => {
  return await get<ListRes<UserData>>("/sys/user/page", { params });
};

// 登录加密密钥（新增/重置密码前获取，用于加密口令）
export const getUserCryptoKey = async () => {
  return await get<string>("/auth/access/getCryptoKey");
};

// 新增（password 为 RSA(AES(...)) 密文，cryptoKey 为密钥信封）
export const createUser = async (
  data: UserData & { password?: string; cryptoKey?: string }
) => {
  return await post("/sys/user/add", data);
};

// 修改（昵称）
export const editUser = async (data: UserData) => {
  return await post("/sys/user/upd", data);
};

// 删除（ids 传用户名）
export const deleteUser = async (username: number | string) => {
  return await get("/sys/user/del", { params: { ids: username } });
};

// 重置密码（password 为 RSA(AES(...)) 密文，cryptoKey 为密钥信封）
export const resetUserPwd = async (data: {
  username: string;
  password: string;
  cryptoKey: string;
}) => {
  return await post("/sys/user/resetPwd", data);
};
