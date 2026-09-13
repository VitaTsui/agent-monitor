import { get } from "../Axios";

// 获取菜单列表
export interface MenuListData {
  topMenuList: MenuList[];
  menuList: MenuList[];
  topId: null;
  topList: null;
}
export interface MenuList {
  id: string;
  nm: string;
  pid: string | null;
  seq: number;
  level: number;
  children: MenuList[] | null;
  path: string;
  url: string;
  perm: string;
  /**
   * 菜单图标的**语义 key**（`user` / `version`），不是图标名。
   * 具体画哪枚图标由 `src/router/menuIcons.ts` 的映射表决定 —— 图标名必须留在源码里，
   * 构建期的图标子集扫描才收得到它（从接口下发的名字扫不到，会在运行时去外网现拉）。
   * 映射查不到时有兜底图标，菜单项不会变成看不见的一行。
   */
  icon: string;
  status: number | null;
}
export const getMenuList = async (
  params: { project: number } = { project: 0 }
) => {
  return await get<MenuListData>("/sys/menu/getMenuATopATopMenu", { params });
};

// 获取权限信息
export interface PermissionsInfo {
  stringPermissions: string[];
}
export const getPermissions = async (
  params: { project: number } = { project: 0 }
) => {
  return await get<PermissionsInfo>("/sys/menu/getStringPermissions", {
    params,
  });
};
