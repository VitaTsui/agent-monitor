import { get } from "../Axios";
import { ListRes } from "../ResType";

export interface EnumItem {
  cd: number;
  nm: string;
}
export interface EnumList {
  list: EnumItem[];
}
export const getEnum = async (cls: string) => {
  return await get<EnumList>("/sys/enum/list", { params: { cls } });
};

export const getEnumByPcd = async (cd: string) => {
  return await get<EnumList>("/sys/cat/listByPcd", { params: { cd } });
};

export const getEnumPageByPcd = async (cd: string, query: string) => {
  return await get<ListRes<EnumItem>>("/sys/cat/pageByPcd", {
    params: { cd, query },
  });
};

/**
 * 角色列表
 */
interface RoleItem {
  id: string | number;
  pid: null;
  cd: string;
  nm: string;
  rmks: string;
  status: number;
  type: number;
  tntId: string;
  crtTm: string;
  crtBy: string;
  updTm: string;
  updBy: string;
  statusDsr: string;
  typeDsr: string;
  rscoIdList: Array<string | number>;
}
export interface RoleList {
  list: RoleItem[];
}
export const getRoleList = async () => {
  return await get<RoleList>("/sys/role/list");
};

// 用户列表
export const getUserList = async () => {
  return await get<ListRes<{ id: string | number; nickname: string }>>(
    "/sys/user/list",
  );
};

// 短信配置列表
export const getSmsConfigList = async () => {
  return await get<ListRes<{ id: string | number; nm: string }>>(
    "/sys/smsConf/list",
  );
};

// 短信模板列表
export const getSmsTemplateList = async () => {
  return await get<ListRes<{ cd: string | number; nm: string }>>(
    "/sys/smsTemplate/list",
  );
};
