/**************************************
 * Module Name : 前台-任务监控对话页
 **************************************/

import { get, post, del } from "@/services/Axios";

import { ListRes } from "@/services/ResType";

import { inDesktopClient } from "@/utils/clientAuth";

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
  /**
   * 这条是在回答终端弹出的选择卡（选项序号或自定义答案）。
   * 单看内容说明不了什么（孤零零一个「1」），问题本身又不在流里 —— 不进对话流。
   */
  fromSelect?: boolean;
}

/** AskUserQuestion 的一道题 */
export interface SelectQuestion {
  question?: string;
  header?: string;
  options?: { label?: string; description?: string }[];
}

/** AskUserQuestion 的整份 input：终端此刻在等你回答的东西 */
export interface SelectPayload {
  questions?: SelectQuestion[];
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
  /**
   * 号位：钉钉里「@N 内容」用的编号，由 hub 按终端锚分配、跨重启保持不变。
   * 网页/客户端显示同一个号，用户才能在手机上照着网页说「@3 继续」。
   * 未参与编号（如占位任务）时为空。
   */
  slot: number | null;
  /** 终端里 claude 原生排队、尚未被接受执行的输入（按入队顺序） */
  queuedInputs: string[];
  /**
   * 终端**此刻正等你选**：AskUserQuestion 的整份 input（questions/options）。
   *
   * 由 PreToolUse hook 在选项弹给终端用户**之前**报上来，所以远端能同步弹出选项框、
   * 替终端做决定。（对话流里的 select 消息是事后从 jsonl 读到的 —— 等它出现时，
   * 人早在终端上选完了，那份只能当记录看。）用户选完即由后续 hook 清除。
   */
  pendingSelect?: SelectPayload;
}
export type PortalTaskData = Partial<IPortalTaskData>;

export type PortalControlAction = "pause" | "resume" | "interrupt" | "stop" | "kill";

/** 当前登录用户信息（含实时 isSuper）；前端每次加载调一次刷新本地缓存，改权限无需重登 */
export interface MeInfo {
  id: string;
  username: string;
  nickname: string;
  isSuper: boolean;
}
export const getMe = async () => {
  return await get<MeInfo>("/monitor/me");
};

// 会话列表（平铺参数过滤）
export const getPortalTaskList = async (params?: {
  status?: string;
  keyword?: string;
}) => {
  return await get<ListRes<PortalTaskData>>("/monitor/tasks", { params });
};

/** 一条远程交互记录：我发的指令，或它给回的结果 */
export interface SessionHistoryItem {
  id: string;
  owner: string;
  /** 所属会话（jsonl id），同一会话的往来会连成一串 */
  sessionId: string;
  /** user = 我下发的；assistant = 它给回的 */
  role: "user" | "assistant";
  content: string;
  /** 发生时刻（epoch 秒） */
  at: number;
  /** 下发来源：dingtalk / web / mcp；assistant 条为空 */
  source: string;
  /** 会话在钉钉里的号位（@N 的 N），终端关太久被回收则为 null */
  slot: number | null;
  hostname: string;
  project: string;
  title: string;
  provider: string;
}

/**
 * 远程交互历史：把「我发了什么 → 它回了什么」按时间排成一条对话流。
 * 返回最近 limit 条，且保持正序（旧 → 新），直接从上往下渲染即是聊天记录的读法。
 *
 * @param session 只看某个会话的往来；不传则返回该账号的全部
 */
export const getSessionHistory = async (limit?: number, session?: string) => {
  return await get<ListRes<SessionHistoryItem>>("/monitor/history", {
    params: { limit, session },
  });
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
    // source 让 hub 区分「客户端 / 网页」下发来源，用于钉钉推送正文标注
    { text, pid, source: inDesktopClient() ? "client" : "web" }
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
  // 显式传文件名（UTF-8 文本字段）：multipart 的 Content-Disposition filename 对非 ASCII
  // （如粘贴图片的「粘贴-xxx.png」）编码在服务端会被解歪，导致落盘名与回填名对不上。
  form.append("name", file.name);
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
  /** 自己的钉钉机器人（一个账号一个，谁配的就服务谁） */
  dingtalk?: {
    appKey: string;
    hasSecret: boolean;
    /** 已经跟机器人说过话 = 它知道该把推送发给谁了 */
    linked: boolean;
  };
  /** 管理员配的公共机器人：没配自己机器人的账号，绑个钉钉号就能用 */
  globalBot?: {
    available: boolean;
    boundIds: DingtalkBoundId[];
  };
  /** 机器人文件接收目录（所有渠道通用）：按「设备 → 项目」层级列出 */
  recvDirDevices?: {
    machineId: string;
    hostname: string;
    projects: {
      cwd: string;
      name: string;
      dir?: string | null;
      taskId?: string | null;
    }[];
  }[];
}

/** 已绑到本账号的钉钉号 */
export interface DingtalkBoundId {
  staffId: string;
  nick: string;
}

export const getIntegrations = async () => {
  return await get<IntegrationsInfo>("/monitor/integrations");
};

/** 设置某项目的钉钉文件接收目录（dir 空 = 清除，回落默认 tmp） */
export const setDingtalkRecvDir = async (project: string, dir: string) => {
  return await post<{ result: string }>(
    "/monitor/integrations/dingtalk-recv-dir",
    { project, dir }
  );
};

/** 配置自己的钉钉机器人；appKey 传空 = 解绑。appSecret 留空表示沿用已存的 */
export const setDingtalkApp = async (data: { appKey: string; appSecret?: string }) => {
  return await post<{ result: string }>(
    "/monitor/integrations/dingtalk-app",
    data,
  );
};

/**
 * 取扫码绑定的授权地址：把 url 画成二维码，用钉钉扫一下即可把该钉钉号绑到本账号。
 * command 是同一个码的另一种用法（扫不了时手动发给机器人）。
 */
export const getDingtalkQr = async () => {
  return await get<{
    url: string;
    code: string;
    expiresIn: number;
    command: string;
  }>("/monitor/integrations/dingtalk-qr");
};

/** 本账号已绑定的钉钉号 */
export const getDingtalkIds = async () => {
  return await get<ListRes<DingtalkBoundId>>("/monitor/integrations/dingtalk-ids");
};

/** 解绑自己的某个钉钉号 */
export const unbindDingtalkId = async (staffId: string) => {
  return await post<{ result: string }>("/monitor/integrations/dingtalk-unbind", {
    staffId,
  });
};

/** 认领机器人回发的绑定链接（URL 上的 ?dtbind= token） */
export const claimDingtalkBind = async (token: string) => {
  return await post<{ result: string; staffId: string; nick: string }>(
    "/monitor/integrations/dingtalk-bind",
    { token },
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

/** 向终端注入按键：撤回终端原生排队(up，按 count 次) / 插入排队到会话(esc)。
 *  仅 iTerm2(mac) 与 Windows 控制台可干净注入。 */
export const termKeyTask = async (
  id: string,
  key: "up" | "esc",
  count = 1,
) => {
  return await post<boolean>(`/monitor/tasks/${id}/termkey`, { key, count });
};

/** 会话目录下的子目录与文件（异步：pending=true 时轮询重试） */
export const getTaskDirs = async (id: string, rel: string) => {
  return await get<{ dirs: string[]; files: string[]; cwd: string; pending: boolean }>(
    `/monitor/tasks/${id}/dirs`,
    { params: { rel } },
  );
};

/** 会话目录内文件夹操作（新建/删除/重命名）：下发给 agent，返回 opId 后轮询结果 */
export const fsopTask = async (
  id: string,
  body: { op: "mkdir" | "delete" | "rename"; rel: string; name: string; newName?: string },
) => {
  return await post<{ opId: string; pending: boolean }>(
    `/monitor/tasks/${id}/fsop`,
    body,
  );
};

/** 取文件夹操作结果（agent 回传前 pending=true，需轮询） */
export const getFsopResult = async (id: string, opId: string) => {
  return await get<{ ok?: boolean; msg?: string; pending: boolean }>(
    `/monitor/tasks/${id}/fsop/${opId}`,
  );
};
