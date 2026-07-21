import {
  getQueuedInputs,
  recallPortalInput,
  termKeyTask,
  PortalControlAction,
  PortalDevice,
  PortalMessage,
  PortalTaskData,
  controlPortalTask,
  deletePortalDevice,
  getPortalDevices,
  getPortalTaskList,
  getPortalTaskMessages,
  sendPortalInput,
  trustPortalDevice,
  untrustPortalDevice,
} from "@/services/apis/portal";

import { makeAutoObservable } from "mobx";
import { message as antdMessage } from "antd";
import { getAccessToken } from "@/utils/auth";

/** 拆分视图最多同时打开的会话数 */
const MAX_PANES = 4;

/** WS 断开后的重连退避（毫秒），逐次递增，封顶 10s */
const WS_RETRY_MS = [1000, 2000, 5000, 10000];

/**
 * WS 掉线时的轮询兜底间隔。
 * WS 正常时不轮询任务列表（推送即时且省流），断了才退回轮询，
 * 保证浏览器/代理不支持 WS 时功能不残废。
 */
const POLL_MS = 2000;

/** 一个设备下的终端类型分组 */
export interface TermGroup {
  /** 分组键：proj-<项目路径>（按项目名分组） */
  key: string;
  /** 组标题 = 项目目录名 */
  title: string;
  tasks: PortalTaskData[];
}

/** 侧栏一台设备 */
export interface DeviceGroup {
  machineId: string;
  hostname: string;
  platform: string;
  platformDsr: string;
  online: boolean;
  groups: TermGroup[];
}

/** 单会话在前端保留的最大消息数（合并是只增不减的，须有上限） */
const MAX_MESSAGES_PER_TASK = 500;

/**
 * 无缓存时统一返回这一个空数组。
 * 每次返回新的 [] 会让消费方 useEffect 的 [messages] 依赖永远比不相等，
 * 空会话下每帧都重跑副作用。
 */
const EMPTY_MESSAGES: PortalMessage[] = [];

class PortalStore {
  private _tasks: PortalTaskData[] = [];
  private _devices: PortalDevice[] = [];
  /** 顶部选中的设备 */
  private _selectedMachineId = "";
  /** 拆分视图中打开的会话（有序） */
  private _openIds: string[] = [];
  /** 各设备各自打开的会话：切设备时不清空、切回来能恢复，避免切设备重置内容界面 */
  private _openIdsByDevice: Record<string, string[]> = {};
  private _messagesById: Record<string, PortalMessage[]> = {};
  private _loadingIds: string[] = [];
  private _keyword = "";
  /**
   * 撤回后把原文回填给对应会话的对话框：Composer 用 reaction 监听，命中自己的
   * taskId 就把 text 填进输入框再消费掉。带 nonce 是为了「撤回同一段文本」也能
   * 重新触发（否则相同对象引用不变、reaction 不响应）。
   */
  composerRefill: { taskId: string; text: string; nonce: number } | null = null;
  private _refillNonce = 0;

  constructor() {
    // 连接管理字段是命令式状态（WS 句柄、定时器句柄、世代号、关闭标志），
    // 没有任何视图观测它们，纳入 observable 既多余、又会在 WS 回调（裸闭包、
    // 非 action）里写入时留下隐患：一旦日后开启 enforceActions 就会抛错。
    // 这里显式排除，让它们保持普通字段。
    makeAutoObservable<
      PortalStore,
      | "_ws"
      | "_wsRetry"
      | "_wsRetryTimer"
      | "_pollTimer"
      | "_deviceTimer"
      | "_wsClosing"
      | "_wsGen"
    >(this, {
      _ws: false,
      _wsRetry: false,
      _wsRetryTimer: false,
      _pollTimer: false,
      _deviceTimer: false,
      _wsClosing: false,
      _wsGen: false,
    });
  }

  private get filtered() {
    const k = this._keyword.trim().toLowerCase();
    if (!k) {
      return this._tasks;
    }

    return this._tasks.filter(
      (t) =>
        (t.projectName ?? "").toLowerCase().includes(k) ||
        (t.prompt ?? "").toLowerCase().includes(k) ||
        (t.hostname ?? "").toLowerCase().includes(k)
    );
  }

  /** 顶部设备选择器的设备列表（有会话的设备，去重） */
  get deviceList(): {
    machineId: string;
    hostname: string;
    platform: string;
    platformDsr: string;
    count: number;
    running: number;
  }[] {
    const byDevice = new Map<string, PortalTaskData[]>();
    this._tasks.forEach((t) => {
      const key = t.machineId || t.hostname || "unknown";
      const list = byDevice.get(key) ?? [];
      list.push(t);
      byDevice.set(key, list);
    });
    const out = Array.from(byDevice.entries()).map(([machineId, list]) => ({
      machineId,
      hostname: list[0]?.hostname ?? machineId,
      platform: list[0]?.platform ?? "",
      platformDsr: list[0]?.platformDsr ?? "",
      count: list.length,
      running: list.filter((t) => t.status === "running").length,
    }));
    out.sort((a, b) => a.hostname.localeCompare(b.hostname));
    return out;
  }

  /** 客户端窗口里由页面注入的本机 machineId（浏览器为空） */
  private _localMachineId = "";

  public setLocalMachineId = (id: string) => {
    this._localMachineId = id;
  };

  get selectedMachineId() {
    // 选中优先级：手动选择 > 本机（仅客户端窗口注入了本机 id 时）。
    // 非本机端（浏览器/远程）进来不自动选任何设备 —— 展示设备列表让用户自己挑，
    // 不再回退到「第一个设备」。
    const list = this.deviceList;
    if (this._selectedMachineId && list.some((d) => d.machineId === this._selectedMachineId)) {
      return this._selectedMachineId;
    }
    if (this._localMachineId && list.some((d) => d.machineId === this._localMachineId)) {
      return this._localMachineId;
    }
    return "";
  }

  public selectMachine = (id: string) => {
    const prev = this.selectedMachineId;
    if (id === prev) {
      return;
    }
    // 切设备不重置内容界面：先存下当前设备打开的会话，切到目标设备时恢复它上次打开的
    // （首次进入则空态）。内容缓存也不清——切回来立即有内容，无需重新拉取白屏一下。
    if (prev) {
      this._openIdsByDevice[prev] = this._openIds;
    }
    this._selectedMachineId = id;
    this._openIds = this._openIdsByDevice[id] ?? [];
    // 该设备没有记忆的打开项时，自动打开一个最近活跃的会话，保证内容区「继续显示着」、
    // 不因切设备而空白。
    if (this._openIds.length === 0) {
      const first = this._tasks
        .filter(
          (t) =>
            (t.machineId || t.hostname || "unknown") === id &&
            t.status !== "finished",
        )
        .sort((a, b) => (b.mtimeMs ?? 0) - (a.mtimeMs ?? 0))[0];
      if (first?.id) {
        this._openIds = [first.id];
      }
    }
  };

  /**
   * 所选设备的会话按「项目名」分组：同一项目下的会话（无论跑在 Cursor、
   * VSCode 还是外部终端）归在一起，组标题就是项目目录名。
   * Map 保持插入序 —— 列表本身按活跃度排序，最活跃的项目自然靠前。
   */
  get selectedGroups(): TermGroup[] {
    const mid = this.selectedMachineId;
    const list = this.filtered.filter(
      (t) =>
        (t.machineId || t.hostname || "unknown") === mid &&
        // 隐藏已结束会话，避免旧会话堆积；但只隐藏「很久没活动」的 ——
        // 配对偶有误判时，最近活动过的会话即使被判 finished 也保留显示，
        // 免得把正在用的终端会话误藏掉。
        (t.status !== "finished" ||
          Date.now() - (t.mtimeMs ?? 0) < 2 * 3600 * 1000)
    );
    // 归一化目录键：与后端 encode_path（core/scanner）逐字对齐 —— 去尾随分隔符后，
    // 把每个非字母数字字符一律替换成 '-'，再小写。同一目录下的空会话（占位任务用进程
    // cwd）与真实会话（用 jsonl 里的 cwd），以及 cursor / 非 cursor 终端，其 cwd 字符串
    // 常在分隔符、盘符冒号、标点等处有细微差异；只做「斜杠/大小写」归一挡不住，必须与
    // 配对键同规则，才能保证「后端认作同一目录、就分进同一个分组」。
    const normProj = (p: string | undefined) =>
      (p ?? "")
        .replace(/[/\\]+$/, "")
        .replace(/[^a-zA-Z0-9]/g, "-")
        .toLowerCase();
    const byProject = new Map<string, TermGroup>();
    for (const t of list) {
      // 组标题只显示文件夹名，不要完整路径
      const dirName = (t.project ?? "").split(/[\\/]/).filter(Boolean).pop() ?? "";
      const title = t.projectName || dirName || "未知项目";
      const key = `proj-${normProj(t.project) || title.toLowerCase()}`;
      const group = byProject.get(key);
      if (group) {
        group.tasks.push(t);
      } else {
        byProject.set(key, { key, title, tasks: [t] });
      }
    }
    // 固定字母序：分组按标题、组内会话按标题(再退 id)稳定排序 —— 之前顺序跟随
    // filtered 的活跃度，活跃会话一变就整列上下跳；改成字母序后位置钉死不乱跳。
    const groups = [...byProject.values()];
    const taskKey = (t: PortalTaskData) =>
      t.title || t.prompt || t.projectName || t.id || "";
    groups.sort((a, b) => a.title.localeCompare(b.title, "zh"));
    for (const g of groups) {
      g.tasks.sort((a, b) => {
        const c = taskKey(a).localeCompare(taskKey(b), "zh");
        return c !== 0 ? c : (a.id ?? "").localeCompare(b.id ?? "");
      });
    }
    return groups;
  }

  get devices() {
    return this._devices;
  }

  get pendingCount() {
    return this._devices.filter((d) => !d.trusted).length;
  }

  /** 全量会话列表（分享接收等场景选目标用） */
  get tasks() {
    return this._tasks;
  }

  get openIds() {
    return this._openIds;
  }

  get openTasks() {
    return this._openIds
      .map((id) => this._tasks.find((t) => t.id === id))
      .filter(Boolean) as PortalTaskData[];
  }

  get keyword() {
    return this._keyword;
  }

  messagesOf = (id: string): PortalMessage[] => this._messagesById[id] ?? EMPTY_MESSAGES;

  isLoadingMessages = (id: string): boolean => this._loadingIds.includes(id);

  setKeyword = (k: string) => {
    this._keyword = k;
  };

  public init = () => {
    this.refresh();
    this.loadDevices();
    this.stopPolling();
    this.connectWs();
    // 设备列表不走 WS（推送只含会话），单独低频轮询
    this._deviceTimer = setInterval(this.loadDevices, 5000);
  };

  // ---------- 实时推送 ----------

  private _ws: WebSocket | null = null;
  private _wsRetry = 0;
  private _wsRetryTimer: ReturnType<typeof setTimeout> | null = null;
  /** WS 不可用时的兜底轮询 */
  private _pollTimer: ReturnType<typeof setInterval> | null = null;
  private _deviceTimer: ReturnType<typeof setInterval> | null = null;
  /** 主动关闭时置位，避免 onclose 触发重连 */
  private _wsClosing = false;
  /**
   * 连接世代号。PortalStore 是单例，close() 又是异步的：
   * 拆卸后组件若重新挂载（路由往返 / StrictMode），新连接会把 _wsClosing 重置，
   * 此时旧连接迟到的 onclose 会误判「不是主动关闭」而触发重连 → 双连接。
   * 每条连接记住自己建立时的世代，回调里比对，过期的直接退出。
   */
  private _wsGen = 0;

  private connectWs = () => {
    const token = getAccessToken();
    if (!token) {
      // 没登录态就别连了，直接退回轮询
      this.startPolling();
      return;
    }
    this._wsClosing = false;
    // 本条连接的世代号：所有回调里比对，认出自己是不是已经被拆卸/替换掉的旧连接
    const gen = ++this._wsGen;
    // 与 API 同源同前缀：开发走 /api 代理（已开 ws:true），生产 API_BASE 为空即同源
    const base = process.env.API_BASE ?? "";
    const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
    const url = `${proto}//${window.location.host}${base}/monitor/ws?token=${encodeURIComponent(
      token,
    )}`;

    let ws: WebSocket;
    try {
      ws = new WebSocket(url);
    } catch {
      this.startPolling();
      return;
    }
    this._ws = ws;

    ws.onopen = () => {
      if (gen !== this._wsGen) {
        return;
      }
      this._wsRetry = 0;
      // 推送已接管会话列表，停掉兜底轮询
      this.stopPolling_();
    };

    ws.onmessage = (e) => {
      if (gen !== this._wsGen) {
        return;
      }
      try {
        const payload = JSON.parse(e.data);
        if (payload?.type === "tasks" && Array.isArray(payload.data)) {
          this.applyTasks(payload.data as PortalTaskData[]);
          return;
        }
        if (payload?.type === "error") {
          // 登录态失效等：服务端会随后断开，这里不重连，退回轮询让
          // 正常的 401 处理去接管跳登录
          this._wsClosing = true;
          this.startPolling();
        }
      } catch {
        // 坏帧忽略，不影响后续推送
      }
    };

    ws.onerror = () => {
      // onerror 后必定跟 onclose，重连逻辑统一放那儿
      ws.close();
    };

    ws.onclose = () => {
      // 过期连接（已被拆卸或新连接替换）的迟到 onclose：什么都不做，
      // 尤其不能重连——否则会和当前连接叠成双连接。
      if (gen !== this._wsGen) {
        return;
      }
      this._ws = null;
      if (this._wsClosing) {
        return;
      }
      // 断线期间先用轮询顶着，别让页面停更
      this.startPolling();
      const delay = WS_RETRY_MS[Math.min(this._wsRetry, WS_RETRY_MS.length - 1)];
      this._wsRetry += 1;
      this._wsRetryTimer = setTimeout(this.connectWs, delay);
    };
  };

  /** 兜底轮询（WS 正常时不跑） */
  private startPolling = () => {
    if (this._pollTimer) {
      return;
    }
    this._pollTimer = setInterval(this.refresh, POLL_MS);
  };

  private stopPolling_ = () => {
    if (this._pollTimer) {
      clearInterval(this._pollTimer);
      this._pollTimer = null;
    }
  };

  /** 页面卸载时的整体拆卸：断开推送、清掉全部定时器（沿用原有调用名） */
  public stopPolling = () => {
    this._wsClosing = true;
    // 推进世代号：当前连接迟到的 onclose 会认出自己已过期而不重连
    this._wsGen += 1;
    if (this._wsRetryTimer) {
      clearTimeout(this._wsRetryTimer);
      this._wsRetryTimer = null;
    }
    this._ws?.close();
    this._ws = null;
    this.stopPolling_();
    if (this._deviceTimer) {
      clearInterval(this._deviceTimer);
      this._deviceTimer = null;
    }
  };

  /**
   * 落地一份会话列表快照。WS 推送与兜底轮询共用，保证两条通路行为一致。
   */
  private applyTasks = (list: PortalTaskData[]) => {
    // /clear、compact 等会让同一终端进程换新会话（新 id/jsonl）：pid 会从旧会话挪到
    // 新会话。先记下旧表里各会话的 pid，换表后把「丢了 pid 的打开会话」跟随到「现在
    // 持有该 pid 的会话」——这样 /clear 后仍能对着同一终端发任务、看内容，而不是卡在
    // 已失联（无 pid）的旧会话上，导致「下发失败、终端没这个任务」。
    const prevPidById = new Map<string, number | null | undefined>();
    for (const t of this._tasks) prevPidById.set(t.id ?? "", t.pid);

    // 内容没变就不换引用，否则整棵会话树白重渲染一遍。
    if (JSON.stringify(list) !== JSON.stringify(this._tasks)) {
      this._tasks = list;
    }

    // pid 跟随：打开的会话若丢了 pid，切到现在持有其原 pid 的会话（同一终端的新会话）
    const followed = this._openIds.map((id) => {
      const cur = this._tasks.find((t) => t.id === id);
      const prevPid = prevPidById.get(id);
      if (cur && !cur.pid && prevPid) {
        const succ = this._tasks.find((t) => t.pid === prevPid && t.id !== id);
        if (succ?.id) return succ.id;
      }
      return id;
    });
    // 跟随后可能与已打开的会话撞车，去重保序
    const deduped = Array.from(new Set(followed));
    if (deduped.join(" ") !== this._openIds.join(" ")) {
      this._openIds = deduped;
    }

    // 仅做存活清理：已消失的会话从打开列表里剔除。
    const alive = this._openIds.filter((id) =>
      this._tasks.some((t) => t.id === id)
    );
    if (alive.length !== this._openIds.length) {
      this._openIds = alive;
      this.dropMessageCache();
    }
    // 对话内容不走推送（推送只含会话列表），仍按需拉取（跟随后的新会话首拉即有内容）
    this._openIds.forEach((id) => this.fetchMessages(id, false));
  };

  public refresh = () => {
    getPortalTaskList()
      .then((res) => {
        if (res.code === 0) {
          this.applyTasks(res.data?.list ?? []);
        }
      })
      .catch(() => {});
  };

  public loadDevices = () => {
    getPortalDevices()
      .then((res) => {
        if (res.code === 0) {
          const list = res.data?.list ?? [];
          // 同 refresh：内容没变就保持引用，避免 2s 一次的无谓重渲染
          if (JSON.stringify(list) !== JSON.stringify(this._devices)) {
            this._devices = list;
          }
        }
      })
      .catch(() => {});
  };

  public trustDevice = (id: string) => {
    trustPortalDevice(id).then((res) => {
      if (res.code === 0) {
        antdMessage.success("已信任该设备");
        this.loadDevices();
        this.refresh();
      } else {
        antdMessage.error(res.msg ?? "操作失败");
      }
    }).catch(() => antdMessage.error("信任设备失败，请检查网络"));
  };

  public untrustDevice = (id: string) => {
    untrustPortalDevice(id).then((res) => {
      if (res.code === 0) {
        antdMessage.success("已撤销信任");
        this.loadDevices();
        this.refresh();
      } else {
        antdMessage.error(res.msg ?? "操作失败");
      }
    }).catch(() => antdMessage.error("撤销信任失败，请检查网络"));
  };

  public deleteDevice = (id: string) => {
    deletePortalDevice(id).then((res) => {
      if (res.code === 0) {
        antdMessage.success("已删除设备");
        this.loadDevices();
        this.refresh();
      } else {
        antdMessage.error(res.msg ?? "操作失败");
      }
    }).catch(() => antdMessage.error("删除设备失败，请检查网络"));
  };

  /** 单击会话：替换为单格视图 */
  public select = (id: string) => {
    if (this._openIds.length === 1 && this._openIds[0] === id) {
      return;
    }

    this._openIds = [id];
    // 从多格切回单格时，被挤掉的会话缓存必须一起丢：
    // refresh 只在会话消失时清理，单纯换视图不会触发它。
    this.dropMessageCache();
    this.fetchMessages(id, true);
  };

  /** 拆分：加入右侧网格 */
  public splitOpen = (id: string) => {
    if (this._openIds.includes(id)) {
      return;
    }
    if (this._openIds.length >= MAX_PANES) {
      antdMessage.warning(`最多同时显示 ${MAX_PANES} 个会话`);
      return;
    }

    this._openIds = [...this._openIds, id];
    this.fetchMessages(id, true);
  };

  public closePane = (id: string) => {
    this._openIds = this._openIds.filter((x) => x !== id);
    this.dropMessageCache();
  };

  /** 丢掉已关闭会话的消息缓存：不清理的话这张表只增不减，长时间挂着会一直涨 */
  private dropMessageCache = () => {
    const keep: Record<string, PortalMessage[]> = {};
    this._openIds.forEach((id) => {
      const msgs = this._messagesById[id];
      if (msgs) {
        keep[id] = msgs;
      }
    });
    this._messagesById = keep;
  };

  /**
   * 主动同步某个会话：丢掉本地累积的消息后重新拉取。
   *
   * 平时的轮询走「累积合并」（滑动窗口会丢老消息，故只增不减）。代价是本地
   * 副本一旦与终端实际对不上，后续轮询只会往上叠，自己纠不回来。
   * 这里直接清空重来，作为对不上时的兜底手段。
   */
  public syncMessages = (id: string) => {
    if (!id) {
      return;
    }
    const next = { ...this._messagesById };
    delete next[id];
    this._messagesById = next;
    this.fetchMessages(id, true);
  };

  /** 刷新「仍在排队」状态：排队中的回显被客户端取走后去掉排队标记 */
  private refreshQueued = (id: string) => {
    const msgs = this._messagesById[id] ?? [];
    if (!msgs.some((m) => m.local && m.queued)) {
      return;
    }
    getQueuedInputs(id)
      .then((res) => {
        if (res.code !== 0) {
          return;
        }
        const still = new Set((res.data?.list ?? []).map((x) => x.cmdId));
        const cur = this._messagesById[id] ?? [];
        if (cur.some((m) => m.local && m.queued && !still.has(m.cmdId ?? ""))) {
          this._messagesById = {
            ...this._messagesById,
            [id]: cur.map((m) =>
              m.local && m.queued && !still.has(m.cmdId ?? "")
                ? { ...m, queued: false, delivered: true }
                : m,
            ),
          };
        }
      })
      .catch(() => void 0);
  };

  /** 撤回后把原文回填给对应会话的对话框（供 Composer 监听消费） */
  public consumeComposerRefill = () => {
    this.composerRefill = null;
  };

  /**
   * 向终端注入按键：撤回终端原生排队（up，按 count 次）/ 插入排队到会话（esc）。
   * 仅 iTerm2(mac) 与 Windows 控制台可干净注入；Terminal.app 会失败并提示手动按键。
   */
  public termKey = (id: string, key: "up" | "esc", count = 1) => {
    termKeyTask(id, key, count)
      .then((res) => {
        if (res.code === 0) {
          antdMessage.success(key === "up" ? "已撤回终端排队" : "已插入排队到会话");
        } else {
          antdMessage.warning(res.msg ?? "按键注入失败");
        }
      })
      .catch(() => antdMessage.error("操作失败，请检查网络"));
  };

  /**
   * 一起撤回底部挂载的多条排队任务，并把它们的原文合并回填进对话框。
   * 只有仍在 hub 队列（带 cmdId、还没被终端取走）的能真正撤回；已进终端原生队列的
   * 撤不回（claude 不开放出队），这里只负责把可撤的撤掉、并把全部文本回填供改后再发。
   */
  public recallAllQueued = (id: string, cmdIds: string[], allText: string) => {
    cmdIds.forEach((cmdId) =>
      recallPortalInput(id, cmdId)
        .then((res) => {
          if (res.code === 0) {
            const cur = this._messagesById[id] ?? [];
            this._messagesById = {
              ...this._messagesById,
              [id]: cur.filter((m) => !(m.local && m.cmdId === cmdId)),
            };
          }
        })
        .catch(() => void 0),
    );
    if (allText.trim()) {
      this.composerRefill = {
        taskId: id,
        text: allText,
        nonce: ++this._refillNonce,
      };
    }
    antdMessage.success("已撤回排队任务");
  };

  /** 撤回还在排队的输入（已被终端接收则提示失败并去掉排队标记） */
  public recallInput = (id: string, cmdId: string) => {
    recallPortalInput(id, cmdId)
      .then((res) => {
        const cur = this._messagesById[id] ?? [];
        if (res.code === 0) {
          antdMessage.success("已撤回");
          // 撤下排队回显，同时把原文回填进该会话的对话框，方便改完再发
          const recalled = cur.find((m) => m.local && m.cmdId === cmdId);
          this._messagesById = {
            ...this._messagesById,
            [id]: cur.filter((m) => !(m.local && m.cmdId === cmdId)),
          };
          if (recalled?.content) {
            this.composerRefill = {
              taskId: id,
              text: recalled.content,
              nonce: ++this._refillNonce,
            };
          }
        } else {
          antdMessage.warning(res.msg ?? "已被终端接收，无法撤回");
          this._messagesById = {
            ...this._messagesById,
            [id]: cur.map((m) =>
              m.local && m.cmdId === cmdId ? { ...m, queued: false } : m,
            ),
          };
        }
      })
      .catch(() => antdMessage.error("撤回失败，请检查网络"));
  };

  public fetchMessages = (id: string, showLoading: boolean) => {
    if (!id) {
      return;
    }
    this.refreshQueued(id);
    if (showLoading && !this._loadingIds.includes(id)) {
      this._loadingIds = [...this._loadingIds, id];
    }

    getPortalTaskMessages(id, 200)
      .then((res) => {
        if (!this._openIds.includes(id)) {
          return;
        }
        if (res.code === 0) {
          const incoming = res.data?.list ?? [];
          // 累积合并：拉取是滑动窗口会丢老消息，这里按 key 去重后只增不减，
          // 保证对话流稳定增长、不因窗口滑动丢历史。
          let prev = this._messagesById[id] ?? [];
          // 终端同步回了同内容的 user 消息 → 撤下对应的本地乐观回显，
          // 让真实消息（带终端时间戳）接管，避免同一条显示两遍。
          // 归一化比对：空白差异（换行/缩进/首尾）一律视为同一条
          const norm = (s: string) => s.replace(/\s+/g, " ").trim();
          const incomingUserNorms = incoming
            .filter((m) => m.role === "user")
            .map((m) => norm(m.content));
          // 回显能否被某条同步回来的 user 消息接管：
          // 除了完全相等，还接受「同步内容包含回显全文」——终端把注入的文本
          // 记进 jsonl 时常会带上结构化前后文（工具结果、上下文块等），导致内容
          // 比原始输入更长，只做全等比对会漏判、两条并存。长度阈值挡掉过短回显
          // （如 “ok”）被任意长消息命中的误伤。
          const echoTakenOver = (echoNorm: string) => {
            if (!echoNorm) {
              return false;
            }
            return incomingUserNorms.some(
              (u) =>
                u === echoNorm ||
                (echoNorm.length >= 4 && u.includes(echoNorm)),
            );
          };
          const now = Date.now();
          const withoutEcho = prev.filter((m) => {
            if (!m.local) {
              return true;
            }
            if (echoTakenOver(norm(m.content))) {
              return false;
            }
            // 自愈兜底：已送达终端超 5 分钟仍没等来同步替换（内容被终端改写等
            // 罕见情况），回显转为普通消息幻影没意义，直接撤下防止重复观感
            const age = now - new Date(m.timestamp).getTime();
            if (m.delivered && age > 5 * 60_000) {
              return false;
            }
            return true;
          });
          const echoReplaced = withoutEcho.length !== prev.length;
          prev = withoutEcho;
          // key 带上全文长度，降低同时间戳+同前缀不同消息被误判重复的概率
          const mkey = (m: PortalMessage) =>
            `${m.timestamp}|${m.role}|${m.content.length}|${m.content.slice(0, 60)}`;
          const seen = new Set(prev.map(mkey));
          const fresh = incoming.filter((m) => {
            const k = mkey(m);
            if (seen.has(k)) {
              return false;
            }
            seen.add(k);
            return true;
          });
          // 无新消息就不换引用：轮询每 2s 一次，无条件替换会让整条对话流
          // 每 2s 白重渲染一遍（长会话下明显掉帧）。
          if (fresh.length || echoReplaced) {
            // 单会话上限：只增不减的合并会随长会话无限涨，超出后丢最老的。
            const merged = [...prev, ...fresh];
            this._messagesById = {
              ...this._messagesById,
              [id]: merged.length > MAX_MESSAGES_PER_TASK
                ? merged.slice(-MAX_MESSAGES_PER_TASK)
                : merged,
            };
          }
        }

        this._loadingIds = this._loadingIds.filter((x) => x !== id);
      })
      .catch(() => {
        this._loadingIds = this._loadingIds.filter((x) => x !== id);
      });
  };

  public control = (id: string, action: PortalControlAction) => {
    const task = this._tasks.find((t) => t.id === id);
    if (!task?.id) {
      return;
    }

    controlPortalTask(task.id, action, task.pid).then((res) => {
      if (res.code === 0) {
        antdMessage.success(res.data?.result ?? "操作成功");
        this.refresh();
      } else {
        antdMessage.error(res.msg ?? "操作失败");
      }
    }).catch(() => antdMessage.error("操作失败，请检查网络"));
  };

  /** 向会话发布任务（注入一行输入） */
  public sendInput = (id: string, text: string) => {
    const task = this._tasks.find((t) => t.id === id);
    const content = text.trim();
    if (!task?.id || !content) {
      return Promise.resolve(false);
    }

    return sendPortalInput(task.id, content, task.pid).then((res) => {
      if (res.code === 0) {
        antdMessage.success(res.data?.result ?? "已发送");
        // 乐观回显：发出的内容立即上屏为 user 气泡，
        // 不等终端收到再同步回来（那要好几秒，体感像没发出去）。
        const echo = {
          role: "user",
          content,
          timestamp: new Date().toISOString(),
          local: true,
          cmdId: res.data?.cmdId,
          queued: !!res.data?.cmdId,
        };
        this._messagesById = {
          ...this._messagesById,
          [id]: [...(this._messagesById[id] ?? []), echo],
        };
        setTimeout(() => this.fetchMessages(id, false), 1200);
        return true;
      }

      antdMessage.error(res.msg ?? "发送失败");
      return false;
    }).catch(() => {
      // 必须吞成 false 返回给调用方：Composer 靠返回值决定要不要把
      // 输入框内容还给用户，抛出去会让草稿连同报错一起丢掉。
      antdMessage.error("发送失败，请检查网络");
      return false;
    });
  };
}

export default new PortalStore();
