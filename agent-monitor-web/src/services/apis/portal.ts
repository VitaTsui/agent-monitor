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
  return await post<{ pid: number; result: string }>(
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
