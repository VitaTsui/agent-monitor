import { createContext, useContext } from "react";

export interface PortalUserInfo {
  nickname?: string;
  username?: string;
  isSuper?: boolean;
}

/**
 * 当前登录用户。**壳持有、整棵子树读**。
 *
 * 不用 `useOutletContext`：设置分栏在第二层出口下，outlet context 是逐层传的，
 * 每加一层就要再转发一次。而这份数据的消费方（问候语、账户页、侧栏、身份 pill）
 * 分布在各层，React context 一次注入即可。
 *
 * 不直接各处读 `getUserInfo()`：壳每次加载会调 `/monitor/me` 刷新（含实时 isSuper），
 * 各处自己读 localStorage 的那一份不会因为刷新而重渲染，会停在旧值上。
 */
export const PortalUserContext = createContext<PortalUserInfo>({});

export const usePortalUser = (): PortalUserInfo => useContext(PortalUserContext);
