import wsCache from "@/utils/wsCache";

// ========== AccessTokenKey ==========

const AccessTokenKey = "FZXVM_ACCESS_TOKEN";

// 获取token
export const getAccessToken = () => {
  return wsCache.get(AccessTokenKey);
};

// 设置token
export const setToken = (token: string) => {
  wsCache.set(AccessTokenKey, token);
};

// 删除token
export const removeToken = () => {
  wsCache.delete(AccessTokenKey);
};

// ========== 账号相关 ==========

const UserInfoKey = "FZXVM_USER_INFO";

export const setUserInfo = (userInfo: object) => {
  wsCache.set(UserInfoKey, userInfo);
};

export const getUserInfo = () => {
  return wsCache.get(UserInfoKey) || {};
};

// ========== 后管访问令牌（部署时生成，X-Admin-Token） ==========
// 存 sessionStorage：关闭标签页即失效，每次新会话需重新解锁后管。

const AdminTokenKey = "AM_ADMIN_TOKEN";

export const getAdminToken = () => {
  return sessionStorage.getItem(AdminTokenKey) ?? "";
};

export const setAdminToken = (token: string) => {
  sessionStorage.setItem(AdminTokenKey, token);
};

export const removeAdminToken = () => {
  sessionStorage.removeItem(AdminTokenKey);
};
