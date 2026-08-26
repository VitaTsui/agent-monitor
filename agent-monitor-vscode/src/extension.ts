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
    // 先清掉已不在的终端：onDidCloseTerminal 偶尔不触发，脏条目会让 findTerminal 命中
    // 已关闭的终端、sendText 石沉大海（下发进 outbox 却送不到）。
    const live = new Set(vscode.window.terminals);
    for (const t of [...termPids.keys()]) {
      if (!live.has(t)) termPids.delete(t);
    }
    for (const t of vscode.window.terminals) {
      try {
        const pid = await t.processId;
        // 只存已解析出的真实 pid：processId 未就绪时可能是 undefined/0/-1，存进去会污染
        // 心跳与匹配、且永不纠正（下一轮 refresh 会重试，就绪后再存）。
        if (pid && pid > 0) termPids.set(t, pid);
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
    // 文件名以毫秒时间戳打头，排序即投递顺序。readdir 本身不保证顺序，
    // 而选择卡的作答是一串**有序**按键（选项序号 → Tab → 回车），顺序错了
    // 就等于先回车后选号。
    files.sort();
    for (const f of files) {
      const fp = path.join(outbox, f);
      let cmd: { pid?: number; text?: string; submit?: boolean; ts?: number };
      try {
        cmd = JSON.parse(fs.readFileSync(fp, "utf8"));
      } catch {
        safeUnlink(fp);
        continue;
      }
      // 30s 前没人认领的命令清掉（多为目标终端所在窗口已关）。这也是「下发进 outbox 却没
      // 送到终端」的失败信号：记一条（带本窗口已知终端 pids），下次排查一眼看出目标 pid 到底
      // 有没有被任何窗口认到——区分「没匹配上终端」与「匹配了但 sendText 没到」。
      if (cmd.ts && Date.now() - cmd.ts > 30000) {
        if (typeof cmd.pid === "number") {
          appendLog(
            `命令超时未送达 pid=${cmd.pid}（本窗口终端 pids=[${Array.from(termPids.values()).join(",")}]）`,
          );
        }
        safeUnlink(fp);
        continue;
      }
      if (typeof cmd.pid !== "number") continue;
      const term = findTerminal(cmd.pid);
      if (term) {
        term.show(false);
        const text = String(cmd.text ?? "");
        const submit = cmd.submit !== false;
        if (submit) {
          // 「文本进去了但没发出（只换行）」的根因：长/多行内容会让 claude 进入粘贴态，
          // sendText(text, true) 把文本和提交回车同批发出，回车被并进粘贴而只换行没提交。
          // 修法：先只粘文本、不带回车；等粘贴态吃完后再【单独送一个】回车提交。
          // 关键——只送一个回车（不是补第二个）：若某次首个回车已提交、或后面弹出了
          // 交互式选择/权限确认框，多按一个回车会误触发下一步/误确认选择。单回车最稳。
          term.sendText(bracketed(text), false);
          const delay = Math.min(1200, 300 + text.length / 3);
          setTimeout(() => {
            try {
              term.sendText("", true);
            } catch {
              /* 终端可能已关闭，忽略 */
            }
          }, delay);
        } else {
          term.sendText(bracketed(text), false);
        }
        safeUnlink(fp);
        // 诊断：记录命中的终端，便于排查「下发到错误终端」
        appendLog(
          `sendText → 终端「${term.name}」processId=${cmd.pid}：${text.slice(0, 40)}`,
        );
        // 本轮到此为止：一次只送一条。
        //
        // 选择卡的作答会被拆成一串按键（每题的序号、多选的 Tab、收尾的回车），
        // 而终端那头是个 TUI —— 答完一题要重渲染、翻到下一题，才认得下一个按键。
        // 原先一轮把 outbox 里的全部连着送出去，中间零间隔，后面的按键很可能落在
        // 还没翻过去的上一题上。借 poll 自己的 500ms 周期当节拍，稳且不必另起计时器。
        return;
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

/**
 * 多行内容用 bracketed paste（ESC[200~ … ESC[201~）包住再送。
 *
 * `sendText` 是直接写进 pty 的：文本里的每个 `\n` 到了终端就是一次**回车提交**，
 * 于是一段多行内容会被拆成好几次提交逐条跑掉 —— 钉钉那边刚把几条转发合并成一段，
 * 到这里又被拆回去，合并等于白做。包上 bracketed paste 后，TUI（Claude Code 等）
 * 会把块内换行当普通文本，整段只在末尾那个单独的回车处提交一次。
 *
 * 与原生 TTY 注入的做法保持一致（见 core/src/process.rs 的 inject_tiocsti）。
 * 单行不包：普通 shell 不认这对转义序列时会把它显示成乱码，能不用就不用。
 */
function bracketed(text: string): string {
  return text.includes("\n") ? `\x1b[200~${text}\x1b[201~` : text;
}

function safeUnlink(fp: string) {
  try {
    fs.unlinkSync(fp);
  } catch {
    /* ignore */
  }
}

export function deactivate() {}
