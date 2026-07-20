/**************************************
 * Module Name : 前台-任务监控对话页
 **************************************/

import { get, post, del } from "@/services/Axios";

import { ListRes } from "@/services/ResType";

export interface PortalTaskProcess {
  pid: number;
  agent: string;
  cwd: string;
  ide: string;
  ideName: string;
  startTime: number;
  cpuUsage: number;
  memory: number;
  command: string;
}

export interface PortalMessage {
  role: string;
  content: string;
  timestamp: string;
  /** 本地乐观回显（发送后立即上屏，终端同步回同内容后被替换） */
  local?: boolean;
  /** 回显对应的队列指令 id（撤回用） */
  cmdId?: string;
  /** 仍在 hub 队列排队、还没被客户端取走 */
  queued?: boolean;
  /** 已送达终端（写入其输入队列），等待终端执行；真实消息同步回来后回显被替换 */
  delivered?: boolean;
}

interface IPortalTaskData {
  id: string;
  title: string;
  usedTokens5h: number;
  tokenLimit: number;
  autoPaused: boolean;
  provider: string;
  providerDsr: string;
  project: string;
  projectName: string;
  prompt: string;
  lastAction: string;
  status: string;
  statusDsr: string;
  ideDsr: string;
  pid: number | null;
  machineId: string;
  hostname: string;
  platform: string;
  platformDsr: string;
  startedAt: string | null;
  lastActiveAt: string | null;
  mtimeMs: number;
  lineCount: number;
  version: string | null;
  gitBranch: string | null;
  process: PortalTaskProcess | null;
}
export type PortalTaskData = Partial<IPortalTaskData>;

export type PortalControlAction = "pause" | "resume" | "interrupt" | "stop" | "kill";

// 会话列表（平铺参数过滤）
export const getPortalTaskList = async (params?: {
  status?: string;
  keyword?: string;
}) => {
  return await get<ListRes<PortalTaskData>>("/monitor/tasks", { params });
};

// 会话消息
export const getPortalTaskMessages = async (id: string, limit?: number) => {
  return await get<ListRes<PortalMessage>>(`/monitor/tasks/${id}/messages`, {
    params: { limit },
  });
};

export interface SlashCommand {
  name: string;
  desc: string;
  source: string;
}

// 该会话模型可用的斜杠命令
export const getPortalSlashCommands = async (id: string) => {
  return await get<ListRes<SlashCommand>>(`/monitor/tasks/${id}/slash-commands`);
};

// ---------- git 改动对比（原文件 vs 修改后） ----------

export interface GitFile {
  status: string;
  path: string;
}
export interface GitOverview {
  isRepo: boolean;
  branch: string;
  files: GitFile[];
  diff: string;
  untracked: string[];
  error: string;
}
// 远程会话首次可能 pending（agent 正在跑），前端轮询直到 pending=false
export const getPortalGitDiff = async (id: string) => {
  return await get<{ overview: GitOverview | null; pending: boolean }>(
    `/monitor/tasks/${id}/git-diff`,
  );
};

// 任务控制
export const controlPortalTask = async (
  id: string,
  action: PortalControlAction,
  pid?: number | null
) => {
  return await post<{ pid: number; result: string }>(
    `/monitor/tasks/${id}/control`,
    { action, pid }
  );
};

// 向会话发布任务（注入一行输入）
export const sendPortalInput = async (
  id: string,
  text: string,
  pid?: number | null
) => {
  return await post<{ pid: number; result: string; cmdId?: string }>(
    `/monitor/tasks/${id}/input`,
    { text, pid }
  );
};

// ---------- 设备管理（信任设备）----------

export interface PortalDevice {
  id: string;
  hostname: string;
  platform: string;
  platformDsr: string;
  version: string;
  online: boolean;
  isHub: boolean;
  sessionCount: number;
  runningCount: number;
  owner: string | null;
  trusted: boolean;
  /** 是否是「他人协助码共享给我」的设备 */
  shared: boolean;
}

export const getPortalDevices = async () => {
  return await get<ListRes<PortalDevice>>("/monitor/devices");
};

export const trustPortalDevice = async (id: string) => {
  return await post(`/monitor/devices/${id}/trust`, {});
};

export const untrustPortalDevice = async (id: string) => {
  return await post(`/monitor/devices/${id}/untrust`, {});
};

export const deletePortalDevice = async (id: string) => {
  return await del(`/monitor/devices/${id}`);
};

// ---------- 协助共享（跨用户接入） ----------

export interface ShareInfo {
  code: string;
  temporary: boolean;
  expiresAt: number;
}

export interface ShareCreated extends ShareInfo {
  password: string;
}

/** 查看本设备当前协助码（主人） */
export const getShareInfo = async (id: string) => {
  return await get<ShareInfo | null>(`/monitor/share/${id}`);
};

/** 生成/刷新协助码（主人）；temporary=true 用系统生成的临时密码 */
export const createShare = async (
  id: string,
  temporary: boolean,
  password?: string,
) => {
  return await post<ShareCreated>(`/monitor/share/${id}`, { temporary, password });
};

/** 停止共享（主人） */
export const revokeShare = async (id: string) => {
  return await del(`/monitor/share/${id}`);
};

/** 当前接入的访客（主人） */
export const getShareGuests = async (id: string) => {
  return await get<ListRes<string>>(`/monitor/share/${id}/guests`);
};

/** 踢掉访客（主人） */
export const kickShareGuest = async (id: string, user: string) => {
  return await post(`/monitor/share/${id}/kick`, { user });
};

/** 访客用连接码 + 密码接入他人设备 */
export const connectShare = async (code: string, password: string) => {
  return await post<{ machineId: string }>("/monitor/share/connect", {
    code,
    password,
  });
};

/** 访客断开自己的接入 */
export const disconnectShare = async (machineId: string) => {
  return await post("/monitor/share/disconnect", { machineId });
};



// ---------- 额度（5h token 上限）----------



// ---------- 文件传输到指定设备目录 ----------

export const uploadPortalFile = async (id: string, dir: string, file: File) => {
  const form = new FormData();
  form.append("dir", dir);
  form.append("file", file);
  return await post<{ path?: string; result?: string; size: number }>(
    `/monitor/devices/${id}/upload`,
    form
  );
};

/** 最新版本信息（更新推送用；desktop = hub 版本，android 来自打包 manifest） */
export interface VersionInfo {
  desktop: string;
  /** 桌面端强制更新下限（低于它必须更新才能继续使用） */
  desktopMin: string | null;
  android: string | null;
  /** 移动端强制更新下限 */
  androidMin: string | null;
}

export const getVersionInfo = () => {
  return get<VersionInfo>("/monitor/version");
};

/** 设备配对认领：把客户端展示的配对码绑定到当前登录账号（绑定即信任） */
export const claimPairDevice = (code: string) => {
  return post<{ machineId: string; hostname: string; platform: string }>(
    "/monitor/pair/claim",
    { code }
  );
};

// ---------- 用户自助机器人集成 ----------

export interface IntegrationsInfo {
  dingtalkRobot: {
    webhook: string;
    hasSecret: boolean;
    waiting: boolean;
    finished: boolean;
    newSession: boolean;
    device: boolean;
  } | null;
  wecomApp: {
    corpId: string;
    token: string;
    hasAesKey: boolean;
    callbackUrl: string;
  } | null;
  dingtalkApp: {
    hasSecret: boolean;
    callbackUrl: string;
  } | null;
}

export const getIntegrations = async () => {
  return await get<IntegrationsInfo>("/monitor/integrations");
};

/** 钉钉群机器人（主动推送） */
export const setDingtalkRobot = async (data: {
  webhook: string;
  secret?: string;
  waiting: boolean;
  finished: boolean;
  newSession: boolean;
  device: boolean;
}) => {
  return await post<boolean>("/monitor/integrations/dingtalk-robot", data);
};

export const testDingtalkRobot = async () => {
  return await post<boolean>("/monitor/integrations/dingtalk-robot/test", {});
};

/** 企业微信自建应用（双向），返回专属回调地址 */
export const setWecomApp = async (data: {
  corpId: string;
  token?: string;
  aesKey?: string;
}) => {
  return await post<{ callbackUrl: string | null }>(
    "/monitor/integrations/wecom-app",
    data,
  );
};

/** 钉钉企业应用（双向），返回专属回调地址 */
export const setDingtalkApp = async (data: { appSecret?: string }) => {
  return await post<{ callbackUrl: string | null }>(
    "/monitor/integrations/dingtalk-app",
    data,
  );
};

/** 仍在排队（未被客户端取走）的输入 */
export const getQueuedInputs = async (id: string) => {
  return await get<ListRes<{ cmdId: string; text: string }>>(
    `/monitor/tasks/${id}/queued`,
  );
};

/** 撤回还在排队的输入（已被终端接收则失败） */
export const recallPortalInput = async (id: string, cmdId: string) => {
  return await post<boolean>(`/monitor/tasks/${id}/recall`, { cmdId });
};

/** 会话目录下的子目录与文件（异步：pending=true 时轮询重试） */
export const getTaskDirs = async (id: string, rel: string) => {
  return await get<{ dirs: string[]; files: string[]; cwd: string; pending: boolean }>(
    `/monitor/tasks/${id}/dirs`,
    { params: { rel } },
  );
};
