import {
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

/** 拆分视图最多同时打开的会话数 */
const MAX_PANES = 4;

/** 一个设备下的终端类型分组 */
export interface TermGroup {
  key: "ide" | "external";
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

class PortalStore {
  private _tasks: PortalTaskData[] = [];
  private _devices: PortalDevice[] = [];
  /** 顶部选中的设备 */
  private _selectedMachineId = "";
  /** 拆分视图中打开的会话（有序） */
  private _openIds: string[] = [];
  private _messagesById: Record<string, PortalMessage[]> = {};
  private _loadingIds: string[] = [];
  private _keyword = "";
  /** 折叠状态：折叠的分组 key 集合（设备 key、终端组 key） */
  private _collapsed: Record<string, boolean> = {};
  private _timer: ReturnType<typeof setInterval> | null = null;

  constructor() {
    makeAutoObservable(this);
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

  get selectedMachineId() {
    // 未手动选择或所选设备已消失时，回退到第一个设备
    const list = this.deviceList;
    if (this._selectedMachineId && list.some((d) => d.machineId === this._selectedMachineId)) {
      return this._selectedMachineId;
    }
    return list[0]?.machineId ?? "";
  }

  public selectMachine = (id: string) => {
    this._selectedMachineId = id;
  };

  /**
   * 所选设备的终端分组（项目终端 = Cursor/VSCode 内嵌，外部终端 = 其它），可折叠。
   */
  get selectedGroups(): TermGroup[] {
    const mid = this.selectedMachineId;
    const list = this.filtered.filter(
      (t) => (t.machineId || t.hostname || "unknown") === mid
    );
    const ide = list.filter(
      (t) => t.process?.ide === "cursor" || t.process?.ide === "vscode"
    );
    const external = list.filter(
      (t) => !(t.process?.ide === "cursor" || t.process?.ide === "vscode")
    );
    const groups: TermGroup[] = [];
    if (ide.length) {
      groups.push({ key: "ide", title: "项目终端", tasks: ide });
    }
    if (external.length) {
      groups.push({ key: "external", title: "外部终端", tasks: external });
    }
    return groups;
  }

  get taskCount() {
    return this._tasks.length;
  }

  get runningCount() {
    return this._tasks.filter((t) => t.status === "running").length;
  }

  get devices() {
    return this._devices;
  }

  get pendingCount() {
    return this._devices.filter((d) => !d.trusted).length;
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

  isCollapsed = (key: string) => !!this._collapsed[key];

  toggleCollapse = (key: string) => {
    this._collapsed = { ...this._collapsed, [key]: !this._collapsed[key] };
  };

  messagesOf = (id: string): PortalMessage[] => this._messagesById[id] ?? [];

  isLoadingMessages = (id: string): boolean => this._loadingIds.includes(id);

  setKeyword = (k: string) => {
    this._keyword = k;
  };

  public init = () => {
    this.refresh();
    this.loadDevices();
    this.stopPolling();
    this._timer = setInterval(() => {
      this.refresh();
      this.loadDevices();
    }, 2000);
  };

  public stopPolling = () => {
    if (this._timer) {
      clearInterval(this._timer);
      this._timer = null;
    }
  };

  public refresh = () => {
    getPortalTaskList()
      .then((res) => {
        if (res.code === 0) {
          this._tasks = res.data?.list ?? [];

          if (!this._openIds.length && this._tasks.length) {
            this.select(this._tasks[0].id ?? "");
            return;
          }
          const alive = this._openIds.filter((id) =>
            this._tasks.some((t) => t.id === id)
          );
          if (alive.length !== this._openIds.length) {
            this._openIds = alive;
          }
          this._openIds.forEach((id) => this.fetchMessages(id, false));
        }
      })
      .catch(() => {});
  };

  public loadDevices = () => {
    getPortalDevices()
      .then((res) => {
        if (res.code === 0) {
          this._devices = res.data?.list ?? [];
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
    });
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
    });
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
    });
  };

  /** 单击会话：替换为单格视图 */
  public select = (id: string) => {
    if (this._openIds.length === 1 && this._openIds[0] === id) {
      return;
    }

    this._openIds = [id];
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
  };

  public fetchMessages = (id: string, showLoading: boolean) => {
    if (!id) {
      return;
    }
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
          const prev = this._messagesById[id] ?? [];
          // key 带上全文长度，降低同时间戳+同前缀不同消息被误判重复的概率
          const mkey = (m: PortalMessage) =>
            `${m.timestamp}|${m.role}|${m.content.length}|${m.content.slice(0, 60)}`;
          const seen = new Set(prev.map(mkey));
          const merged = [...prev];
          incoming.forEach((m) => {
            if (!seen.has(mkey(m))) {
              seen.add(mkey(m));
              merged.push(m);
            }
          });
          this._messagesById = { ...this._messagesById, [id]: merged };
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
    });
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
        setTimeout(() => this.fetchMessages(id, false), 1200);
        return true;
      }

      antdMessage.error(res.msg ?? "发送失败");
      return false;
    });
  };
}

export default new PortalStore();
