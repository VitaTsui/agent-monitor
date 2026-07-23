import * as vscode from "vscode";
import * as fs from "fs";
import * as path from "path";
import * as os from "os";

/**
 * 终端任务监控 · 桥接扩展。
 *
 * 内嵌终端走 ConPTY，桌面客户端无法用 WriteConsoleInput 注入；本扩展与客户端通过
 * `<data_dir>/bridge/` 下的文件通信：
 *  - 每 2s 写 `win-<id>.json`（{ts, terminals:[shell_pid...]}）当心跳 + 终端清单；
 *  - 轮询 `outbox/*.json`（{pid, text, submit}），pid 命中本窗口某终端就 sendText 送达并删文件。
 *
 * data_dir 与客户端对齐：Windows = %APPDATA%\AgentMonitor，其它 = ~/.agent-monitor。
 */

function dataDir(): string {
  // 与客户端 default_data_dir()（Rust dirs::data_dir()/AgentMonitor）严格对齐。
  // 客户端也支持环境变量 AM_DATA_DIR 覆盖，这里同样优先取它。
  if (process.env.AM_DATA_DIR) return process.env.AM_DATA_DIR;
  const home = os.homedir();
  if (process.platform === "win32") {
    return path.join(process.env.APPDATA || path.join(home, "AppData", "Roaming"), "AgentMonitor");
  }
  if (process.platform === "darwin") {
    return path.join(home, "Library", "Application Support", "AgentMonitor");
  }
  return path.join(
    process.env.XDG_DATA_HOME || path.join(home, ".local", "share"),
    "AgentMonitor",
  );
}

export function activate(context: vscode.ExtensionContext) {
  const base = path.join(dataDir(), "bridge");
  const outbox = path.join(base, "outbox");
  try {
    fs.mkdirSync(outbox, { recursive: true });
  } catch {
    /* ignore */
  }

  const winId = `${process.pid}`;
  const beat = path.join(base, `win-${winId}.json`);

  // 缓存本窗口各终端的 shell pid（processId 是 Promise，异步刷新）
  const termPids = new Map<vscode.Terminal, number>();
  const refreshPids = async () => {
    for (const t of vscode.window.terminals) {
      try {
        const pid = await t.processId;
        if (pid) termPids.set(t, pid);
      } catch {
        /* ignore */
      }
    }
  };
  const findTerminal = (pid: number): vscode.Terminal | undefined => {
    for (const [t, p] of termPids) if (p === pid) return t;
    return undefined;
  };

  const writeHeartbeat = () => {
    const pids = Array.from(termPids.values());
    try {
      fs.writeFileSync(beat, JSON.stringify({ ts: Date.now(), terminals: pids }));
    } catch {
      /* ignore */
    }
  };

  const poll = () => {
    let files: string[] = [];
    try {
      files = fs.readdirSync(outbox).filter((f) => f.endsWith(".json"));
    } catch {
      return;
    }
    for (const f of files) {
      const fp = path.join(outbox, f);
      let cmd: { pid?: number; text?: string; submit?: boolean; ts?: number };
      try {
        cmd = JSON.parse(fs.readFileSync(fp, "utf8"));
      } catch {
        safeUnlink(fp);
        continue;
      }
      // 30s 前没人认领的命令清掉（多为目标终端所在窗口已关）
      if (cmd.ts && Date.now() - cmd.ts > 30000) {
        safeUnlink(fp);
        continue;
      }
      if (typeof cmd.pid !== "number") continue;
      const term = findTerminal(cmd.pid);
      if (term) {
        term.show(false);
        const text = String(cmd.text ?? "");
        const submit = cmd.submit !== false;
        term.sendText(text, submit);
        // 兜底二次回车（对齐 mac osascript 的 inject_applescript）：长/多行内容会让 claude
        // 进入粘贴态，sendText 补的那个提交回车被并进粘贴而只换行没提交（表现为「任务贴进去
        // 了但没发出」）。等粘贴态吃完后再补一个空回车提交；若首次已提交，此时输入为空，
        // claude 对空回车无动作，安全。延时按内容长度递增（300ms 起，封顶 1.2s）。
        if (submit) {
          const delay = Math.min(1200, 300 + text.length / 3);
          setTimeout(() => {
            try {
              term.sendText("", true);
            } catch {
              /* 终端可能已关闭，忽略 */
            }
          }, delay);
        }
        safeUnlink(fp);
        // 诊断：记录命中的终端，便于排查「下发到错误终端」
        appendLog(
          `sendText → 终端「${term.name}」processId=${cmd.pid}：${text.slice(0, 40)}`,
        );
      }
      // 不是本窗口的终端就留着，交给拥有该终端的窗口处理（TTL 兜底清理）
    }
  };

  const appendLog = (line: string) => {
    try {
      const ts = new Date().toISOString();
      fs.appendFileSync(path.join(base, "ext.log"), `[${ts}] win-${winId} ${line}\n`);
    } catch {
      /* ignore */
    }
  };

  // 起步先刷一次；终端增删时刷新
  void refreshPids().then(writeHeartbeat);
  context.subscriptions.push(
    vscode.window.onDidOpenTerminal(() => void refreshPids().then(writeHeartbeat)),
    vscode.window.onDidCloseTerminal((t) => {
      termPids.delete(t);
      writeHeartbeat();
    }),
  );

  const beatTimer = setInterval(() => void refreshPids().then(writeHeartbeat), 2000);
  const pollTimer = setInterval(poll, 500);
  context.subscriptions.push({
    dispose: () => {
      clearInterval(beatTimer);
      clearInterval(pollTimer);
      safeUnlink(beat);
    },
  });

  context.subscriptions.push(
    vscode.commands.registerCommand("agentMonitorBridge.status", () => {
      vscode.window.showInformationMessage(
        `终端任务监控桥接：监听 ${termPids.size} 个终端，桥接目录 ${base}`,
      );
    }),
  );
}

function safeUnlink(fp: string) {
  try {
    fs.unlinkSync(fp);
  } catch {
    /* ignore */
  }
}

export function deactivate() {}
