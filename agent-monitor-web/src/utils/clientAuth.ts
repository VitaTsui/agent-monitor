import { post } from "@/services/Axios";
import { setToken, setUserInfo } from "@/utils/auth";

interface TauriBridge {
  core?: {
    invoke?: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
  };
}

/** 是否运行在桌面客户端窗口内（浏览器里没有 __TAURI__） */
export const inDesktopClient = (): boolean =>
  !!(window as unknown as { __TAURI__?: TauriBridge }).__TAURI__?.core?.invoke;

/** 是否运行在移动端原生壳（Capacitor）里；浏览器里无 Capacitor 桥 */
export const inMobileApp = (): boolean =>
  !!(
    window as unknown as {
      Capacitor?: { isNativePlatform?: () => boolean };
    }
  ).Capacitor?.isNativePlatform?.();

/** 是否在原生壳（桌面客户端 或 移动端 App）内——用于隐藏只在浏览器里可用的入口（如后台管理） */
export const inNativeShell = (): boolean => inDesktopClient() || inMobileApp();

interface ClientCred {
  machineId?: string;
  deviceToken?: string;
}

/** 本机 machine_id（仅客户端窗口内有值；浏览器返回 null）。结果缓存。 */
let cachedLocalId: string | null | undefined;
export async function localMachineId(): Promise<string | null> {
  if (cachedLocalId !== undefined) {
    return cachedLocalId;
  }
  const invoke = (window as unknown as { __TAURI__?: TauriBridge }).__TAURI__
    ?.core?.invoke;
  if (!invoke) {
    cachedLocalId = null;
    return null;
  }
  try {
    const id = (await invoke("local_machine_id")) as string;
    cachedLocalId = id || null;
  } catch {
    cachedLocalId = null;
  }
  return cachedLocalId;
}

interface SessionRes {
  code: number;
  data?: { token?: string; userInfo?: object };
}

/**
 * 桌面客户端静默续登：用本机持久化的设备令牌换一个新的登录会话。
 *
 * 客户端登录一次完成绑定后，设备令牌永久有效 —— 网页会话过期或服务端
 * 重启时，这里自动换新会话，客户端里的登录态对用户而言就是永不过期。
 * 浏览器环境 / 未绑定设备 / 令牌被撤销时返回 false，调用方回退到登录页。
 */
// 在飞去重 + 失败冷却：续登请求自身 401 也会触发全局 reLogin → 再次续登，
// 不加闸会形成无限循环。失败后 15s 内直接返回 false（不发请求），让调用方
// 走回登录页兜底。
let inflight: Promise<boolean> | null = null;
let lastFailAt = 0;

export function clientSilentLogin(): Promise<boolean> {
  if (inflight) {
    return inflight;
  }
  if (Date.now() - lastFailAt < 15_000) {
    return Promise.resolve(false);
  }
  inflight = doSilentLogin()
    .then((ok) => {
      if (!ok) {
        lastFailAt = Date.now();
      }
      return ok;
    })
    .finally(() => {
      inflight = null;
    });
  return inflight;
}

async function doSilentLogin(): Promise<boolean> {
  const invoke = (window as unknown as { __TAURI__?: TauriBridge }).__TAURI__
    ?.core?.invoke;
  if (!invoke) {
    return false;
  }
  try {
    const cred = (await invoke("client_auth")) as ClientCred | null;
    if (!cred?.machineId || !cred.deviceToken) {
      return false;
    }
    const res = (await post<SessionRes["data"]>("/monitor/client/session", {
      machineId: cred.machineId,
      deviceToken: cred.deviceToken,
    })) as SessionRes;
    if (res.code !== 0 || !res.data?.token) {
      return false;
    }
    setToken(res.data.token);
    if (res.data.userInfo) {
      setUserInfo(res.data.userInfo);
    }
    return true;
  } catch {
    return false;
  }
}
