import { getTaskFile } from "@/services/apis/portal";
import { inDesktopClient, localMachineId } from "@/utils/clientAuth";

/**
 * 会话内容里的本地图片。
 *
 * agent 的输出常带 `![交互提示](qa/evidence/x.png)` 这种**相对路径** —— 那是跑 agent
 * 那台机器上的文件。浏览器会拿它去拼当前站点地址（`https://<hub>/qa/evidence/x.png`）
 * 直接 404，所以四端此前一律是破图。这里把这些路径换成真正能显示的地址。
 *
 * 两条取回通道：
 * - **桌面客户端 + 会话就在本机** → 走 IPC 直接读本地文件（图本来就在手边，不必上传）
 * - 其它情况 → 向 hub 要（hub 再向那台机器现取，只在内存中转、不落盘）
 */
export interface SessionImageCtx {
  taskId: string;
  machineId: string;
  /** 会话工作目录（相对路径以它为根） */
  cwd: string;
}

interface TauriBridge {
  core?: {
    invoke?: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
  };
}

/** 已解析结果缓存：同一张图在对话流里会被反复渲染，不能每次都去取 */
const cache = new Map<string, string>();
/** 取过且失败的，别一遍遍重试（失败多半是文件真的不在） */
const failed = new Set<string>();

const keyOf = (ctx: SessionImageCtx, rel: string) =>
  `${ctx.machineId}|${ctx.cwd}|${rel}`;

/** markdown 图片语法，捕获 alt 与路径。路径里不含空格与右括号（markdown 本身的限制） */
const IMG_RE = /!\[([^\]]*)\]\(([^)\s]+)\)/g;

/** 已经能直接显示的地址：http(s)、data、blob、以及站内绝对路径 */
const isResolvable = (src: string) =>
  /^(https?:|data:|blob:|\/)/i.test(src);

/** 从 markdown 里挑出需要解析的本地图片路径（去重） */
export function localImageRefs(md: string): string[] {
  const out = new Set<string>();
  for (const m of md.matchAll(IMG_RE)) {
    const src = m[2];
    if (!isResolvable(src)) {
      out.add(src);
    }
  }
  return [...out];
}

/** 桌面客户端里读本机文件，回 data URL */
async function readLocal(cwd: string, rel: string): Promise<string | null> {
  const invoke = (window as unknown as { __TAURI__?: TauriBridge }).__TAURI__
    ?.core?.invoke;
  if (!invoke) {
    return null;
  }
  try {
    const url = (await invoke("read_session_image", { cwd, rel })) as string;
    return url || null;
  } catch {
    return null;
  }
}

/** 向 hub 要（hub 再向那台机器现取）。异步取件，所以要轮几次 */
async function fetchViaHub(
  taskId: string,
  rel: string
): Promise<string | null> {
  // 客户端上报周期约 1.5s，多等几轮足够；再久多半是文件不存在，别一直挂着
  for (let i = 0; i < 8; i++) {
    const res = await getTaskFile(taskId, rel).catch(() => null);
    if (!res || res.code !== 0) {
      return null; // 404/离线之类：明确失败，不必再轮
    }
    if (!res.data?.pending) {
      const { mime, contentB64 } = res.data ?? {};
      // 认不出类型的一律不显示 —— 与其把非图片塞进 img，不如留破图
      if (!mime || !contentB64 || !mime.startsWith("image/")) {
        return null;
      }
      return `data:${mime};base64,${contentB64}`;
    }
    await new Promise((r) => window.setTimeout(r, 700));
  }
  return null;
}

/** 取一张图，返回可直接放进 img src 的地址；取不到返回 null */
async function resolveOne(
  ctx: SessionImageCtx,
  rel: string
): Promise<string | null> {
  const key = keyOf(ctx, rel);
  const hit = cache.get(key);
  if (hit) {
    return hit;
  }
  if (failed.has(key)) {
    return null;
  }
  let url: string | null = null;
  // 图就在本机时直接读，省一整趟「客户端 → hub → 网页」的往返
  if (inDesktopClient() && (await localMachineId()) === ctx.machineId) {
    url = await readLocal(ctx.cwd, rel);
  }
  // 本机读不到（不在客户端里、或会话在别的机器上）就让 hub 去现取
  if (!url) {
    url = await fetchViaHub(ctx.taskId, rel);
  }
  if (url) {
    cache.set(key, url);
  } else {
    failed.add(key);
  }
  return url;
}

/**
 * 把 markdown 里的本地图片路径换成可显示的地址。
 *
 * 没有本地图片时**原样返回同一个字符串**（不是新对象）—— 调用方据此可以跳过重渲染，
 * 而对话流里绝大多数消息本来就不含图片。
 */
export async function resolveSessionImages(
  md: string,
  ctx?: SessionImageCtx
): Promise<string> {
  if (!ctx || !md) {
    return md;
  }
  const refs = localImageRefs(md);
  if (refs.length === 0) {
    return md;
  }
  const resolved = new Map<string, string>();
  await Promise.all(
    refs.map(async (rel) => {
      const url = await resolveOne(ctx, rel);
      if (url) {
        resolved.set(rel, url);
      }
    })
  );
  if (resolved.size === 0) {
    return md;
  }
  // 取不到的保持原样：留着破图，至少还看得见它引用了哪个文件
  return md.replace(IMG_RE, (whole, alt: string, src: string) => {
    const url = resolved.get(src);
    return url ? `![${alt}](${url})` : whole;
  });
}
