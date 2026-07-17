`
import { get, post } from "@/services/Axios";

import { ListRes } from "@/services/ResType";

export interface <NAME>SearchData {}

interface <NAME>Search {
  query: string;
}

interface I<NAME>Data {
  id: string | number;
}

export type <NAME>Data = Partial<I<NAME>Data>;

/**
 * 获取列表
 * @param params 查询参数
 */
export const get<NAME>List = async (params: <NAME>Search) => {
  return await get<ListRes<<NAME>Data>>("<API_PAGE_PATH>", { params });
};

/**
 * 获取详情
 * @param id 记录ID
 */
export const get<NAME> = async (id: number | string) => {
  return await get<<NAME>Data>("<API_INFO_PATH>" + id);
};

/**
 * 新增
 * @param data 数据
 */
export const create<NAME> = async (data: <NAME>Data) => {
  return await post("<API_ADD_PATH>", data);
};

/**
 * 修改
 * @param data 数据
 */
export const edit<NAME> = async (data: <NAME>Data) => {
  return await post("<API_UPD_PATH>", data);
};

/**
 * 删除
 * @param id 记录ID
 */
export const delete<NAME> = async (id: number | string) => {
  return await get("<API_DEL_PATH>", { params: { ids: id } });
};
`;
