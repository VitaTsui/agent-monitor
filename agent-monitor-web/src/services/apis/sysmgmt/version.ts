import { get, post } from "@/services/Axios";

/** 一条更新日志 */
export interface ChangelogEntry {
  version: string;
  date: string;
  notes: string;
}

/** 版本管理信息（后管） */
export interface VersionAdminInfo {
  /** 桌面端最新版本（= hub 版本，随发版自动更新） */
  desktop: string;
  /** 桌面端强制更新下限（低于它必须更新才能继续使用） */
  desktopMin: string | null;
  /** 移动端最新版本（打包 manifest） */
  android: string | null;
  /** 移动端强制更新下限 */
  androidMin: string | null;
  changelog: ChangelogEntry[];
}

export const getVersionAdminInfo = () => {
  return get<VersionAdminInfo>("/sys/version/info");
};

/** 设置强制更新下限（写 manifest，全端即刻生效） */
export const setVersionMinimum = (data: {
  desktopMin?: string;
  androidMin?: string;
}) => {
  return post<boolean>("/sys/version/minimum", data);
};

/** 新增/覆盖一条更新日志 */
export const addChangelog = (data: ChangelogEntry) => {
  return post<boolean>("/sys/version/changelog", data);
};

export const delChangelog = (version: string) => {
  return post<boolean>("/sys/version/changelog/del", { version });
};
