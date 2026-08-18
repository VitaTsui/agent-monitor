import React, { useEffect, useRef, useState } from "react";

import { Button } from "@hsu-react/ui";
import { Dropdown, Modal, Popconfirm, Spin, Tooltip } from "antd";
import {
  CloseOutlined,
  CompressOutlined,
  ExpandOutlined,
  MoreOutlined,
  PauseCircleOutlined,
  PlayCircleOutlined,
  StopOutlined,
  SyncOutlined,
  ThunderboltOutlined,
} from "@ant-design/icons";
import { observer } from "mobx-react-lite";

import { PortalMessage, PortalTaskData } from "@/services/apis/portal";
import PortalStore from "../../PortalStore";
import Composer from "../Composer";
import TerminalFeed, { SelectCard } from "../TerminalFeed";
import SessionPanels from "../SessionPanels";
import styles from "./index.module.scss";

interface ChatPaneProps {
  task: PortalTaskData;
  /** 是否显示关闭按钮（多格时） */
  closable?: boolean;
  /**
   * 紧凑只读模式：放大布局下右侧那一列卡片用。
   *
   * 藏掉输入框与排队条 —— 卡片只有固定的一点高度，塞下输入框就没剩多少地方看内容了；
   * 真要发东西，点一下把它换到主区再说。头部的控制按钮仍保留（暂停/中断这类不需要打字）。
   */
  compact?: boolean;
  /** 点卡片本体：放大布局里用来与主区对调 */
  onActivate?: () => void;
}

/** token 数量缩写：1.2k / 3.4M */
const fmtTokens = (n: number) => {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
};

/**
 * 头部按钮平铺所需的最小格宽。低于它就收回 `⋯` 菜单 —— 按钮会把标题挤成几个字，
 * 而「这一格是哪个会话」比「少点一下」重要得多。
 *
 * 两档：多格时头部有 6 个按钮（含放大、关闭），单格只有 4 个，需要的地方自然不同。
 * 数值按「按钮 ~32px + 标题至少留 180px」估，再取整。
 */
const FLAT_MIN_W = { multi: 400, single: 320 };

const ChatPane: React.FC<ChatPaneProps> = observer((props) => {
  const { task, closable, compact, onActivate } = props;
  const {
    messagesOf,
    isLoadingMessages,
    control,
    closePane,
    sendInput,
    recallInput,
    recallAllQueued,
    termKey,
    syncMessages,
    hubQueuedOf,
    focusedId,
    setFocused,
  } = PortalStore;
  const chatRef = useRef<HTMLDivElement>(null);
  const stickBottomRef = useRef(true);
  const rootRef = useRef<HTMLDivElement>(null);
  /** 本格是否窄到摆不下一排按钮（按实测宽度判定，不看视口 —— 决定拥挤的是格宽） */
  const [narrow, setNarrow] = useState(false);

  const id = task.id ?? "";
  const messages = messagesOf(id);
  const hubQueued = hubQueuedOf(id);
  const loading = isLoadingMessages(id);
  // 一条任务的去处，取决于它有没有被终端拿去执行：
  //
  //   还没轮到 → 只在下方排队条，可撤回
  //   已进执行 → 只在对话流，撤不回了（撤回按钮也就不该出现在正文里）
  //
  // 判据以终端上报的 queuedInputs 为准 —— 它随任务被会话接受而出列，是唯一
  // 知道「跑了没有」的一方。本地回显的 delivered 只说明「送出去了」。
  const normText = (s: string) => s.replace(/\s+/g, " ").trim();
  const stillQueued = React.useMemo(() => {
    const set = new Set((task.queuedInputs ?? []).map(normText));
    return (m: PortalMessage) => {
      if (!m.local) {
        return false; // 终端同步回来的真实消息，早已在执行流里
      }
      if (m.queued) {
        return true; // 还在 hub 队列，连终端都没送到
      }
      // 已送达终端：在队列里就是还没轮到。刚送达的几秒终端还没来得及上报，
      // 先按「在队列」处理，免得它在对话流里闪一下又跳回排队条。
      return (
        Date.now() - new Date(m.timestamp).getTime() < 6000 ||
        set.has(normText(m.content))
      );
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [task.queuedInputs]);

  // 会话上下文：内容里的本地图片路径以会话 cwd 为根解析（见 utils/sessionImages）。
  // 没有 cwd（历史会话、进程已退出）就不给 —— 那时路径无从解析，保持破图但不误导。
  const imageCtx = React.useMemo(() => {
    // PortalTask 的字段都是 Partial 的，三者齐全才谈得上解析
    // 与 Composer 同一个根：会话内容里的相对图片路径也是终端按当前目录写下的，
    // 用进程 cwd 解析会在 cd 过的会话里全变破图。
    const cwd = task.liveCwd || task.process?.cwd || "";
    return cwd && task.id && task.machineId
      ? { taskId: task.id, machineId: task.machineId, cwd }
      : undefined;
  }, [task.id, task.machineId, task.liveCwd, task.process?.cwd]);

  const feedMessages = React.useMemo(
    () =>
      messages.filter(
        (m) =>
          m.role !== "todos" &&
          m.role !== "bgtasks" &&
          // 选择卡一律不进内容流：要选的时候它会弹在输入框上方（那张来自 hook，
          // 是「此刻」的真问题、可点可答）。流里再放一张只会是重复的历史副本 ——
          // 长得一模一样却点不得，反而让人分不清哪张才是在等自己。
          // 选完之后答案本身会作为一条 user 消息进流，记录并不会丢。
          m.role !== "select" &&
          // 选择卡的答案不进对话流：孤零零一个「1」「2」看不出在答什么，
          // 而问题本身挂在输入框上方、根本不在流里
          !m.fromSelect &&
          // 还在排队的不进对话流 —— 它归排队条管，在那里才撤得回
          !stillQueued(m),
      ),
    [messages, stillQueued],
  );

  // 底部「排队中」挂载项：本地回显（还在 hub 队列、可撤回）+ 终端里 claude 原生
  // 队列（queued_inputs，已被终端接收、撤不回，仅展示）。按内容去重，本地项优先
  // （带 cmdId 可撤回）。
  const queuedItems = React.useMemo(() => {
    const norm = (s: string) => s.replace(/\s+/g, " ").trim();
    const items: {
      text: string;
      cmdId?: string;
      recallable: boolean;
    }[] = [];
    const seen = new Set<string>();
    // 过长内容的收尾兜底：一旦它作为**真实消息**（非本地回显，m.local 为假）进了对话流，
    // 就说明终端已经把它拿去执行了；此时哪怕终端的 queued_inputs 因超长匹配失败还挂着这
    // 一条，也不该在排队条里重复显示。预先把这些内容塞进 seen，下面三处 push 自然跳过。
    //
    // 只对长内容（>200 字）这么做：短命令（「继续」「y」）完全可能被合法地再次排队，
    // 按内容去重会把这次真正在排队的那条误当成历史消息藏掉。长内容几乎不会一字不差重复，
    // 没有这个误伤风险。
    for (const m of messages) {
      if (!m.local && m.content && m.content.length > 200) {
        seen.add(norm(m.content));
      }
    }
    // 与对话流是互补的两半（判据同 stillQueued）：还没轮到的在这里、可撤回；
    // 一被终端拿去执行就从这里出列、转到对话流，那时也就撤不回了。
    // 同一条任务任何时刻只出现在一处，不会两边都有。
    //
    // 顺序即终端队列的真实先后：先铺 queued_inputs（终端 queue-operation 的真实 FIFO，
    // 最旧在上、最新在下），再把「还在 hub 队列、尚未注入终端」的本地回显（可撤回）挂在
    // 最下（它们是刚发出、最新的）。这样一条任务从「本地回显（挂底）」过渡到「终端原生
    // 队列（同样挂底）」时位置不跳；之前是回显在上、入队后落到下，才有「先在最上、又回到
    // 最下」的闪跳。已注入终端的排队状态一律以 queued_inputs 为准 —— 它在任务被会话接受
    // 时随 remove 出列而清掉。
    for (const t of task.queuedInputs ?? []) {
      const k = norm(t);
      if (!seen.has(k)) {
        seen.add(k);
        items.push({ text: t, recallable: false });
      }
    }
    // hub 队列（还没下发到终端）：这一份是**跨端可见**的，手机上发的任务靠它才能
    // 出现在电脑端。本地回显只活在发起方自己的浏览器里，别的端看不到。
    // 排在终端队列之后、本地回显之前 —— 它比终端里那些新，又比刚发出的旧。
    for (const q of hubQueued) {
      const k = norm(q.text);
      if (!seen.has(k)) {
        seen.add(k);
        items.push({ text: q.text, cmdId: q.cmdId, recallable: true });
      }
    }
    for (const m of messages) {
      // 用同一个判据取「还没轮到的本地回显」：除了还在 hub 队列的（queued，
      // 可撤回），也含刚送达终端、尚未被拿去执行的那几秒 —— 否则这段时间里
      // 它既被对话流挡在外面、又不在排队条上，人就看不到自己刚发的东西了。
      // 选择卡的答案也不进排队条：它是对终端提问的回答、不是待办任务，
      // 而且终端正等着它，转眼就被吃掉，挂在「还没跑的」里只会让人困惑
      if (m.local && !m.fromSelect && stillQueued(m)) {
        const k = norm(m.content);
        if (!seen.has(k)) {
          seen.add(k);
          // 只有还在 hub 队列（有 cmdId）的撤得回；已注入终端的只能整体按 ↑
          items.push({ text: m.content, cmdId: m.cmdId, recallable: !!m.queued });
        }
      }
    }
    return items;
  }, [messages, task.queuedInputs, hubQueued, stillQueued]);

  // 按实际格宽决定头部平铺还是收进菜单。用 ResizeObserver 而非视口断点：同一个视口下
  // 格子可能是 1/2/3/4 等分，还能被侧栏折叠改变，只有量自己才准。
  useEffect(() => {
    const el = rootRef.current;
    if (!el) {
      return;
    }
    const limit = closable ? FLAT_MIN_W.multi : FLAT_MIN_W.single;
    const ro = new ResizeObserver((entries) => {
      const w = entries[0]?.contentRect.width;
      if (w) {
        setNarrow(w < limit);
      }
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [closable]);

  useEffect(() => {
    const el = chatRef.current;
    if (el && stickBottomRef.current) {
      el.scrollTop = el.scrollHeight;
      // Markdown/代码块/字体异步渲染会让高度继续涨，同步滚一次会差一截：
      // 下一帧再钉一次兜住首帧的增量
      requestAnimationFrame(() => {
        const cur = chatRef.current;
        if (cur && stickBottomRef.current) {
          cur.scrollTop = cur.scrollHeight;
        }
      });
    }
  }, [messages]);

  // 内容高度变化（渲染完成、折叠展开等）时，只要用户仍在底部就保持钉底；
  // 用户主动上滚后 stickBottomRef 为 false，不会抢滚动。
  useEffect(() => {
    const el = chatRef.current;
    if (!el) {
      return;
    }
    const ro = new ResizeObserver(() => {
      if (stickBottomRef.current) {
        el.scrollTop = el.scrollHeight;
      }
    });
    ro.observe(el);
    if (el.firstElementChild) {
      ro.observe(el.firstElementChild);
    }
    return () => ro.disconnect();
  }, []);

  const onChatScroll = () => {
    const el = chatRef.current;
    if (el) {
      stickBottomRef.current =
        el.scrollHeight - el.scrollTop - el.clientHeight < 80;
    }
  };

  const paused = task.status === "paused";
  const controllable = !!task.pid;
  const running = task.status === "running";
  // 操作跟状态绑死，而不是一律「有 pid 就能点」：
  // - 中断：只有正跑着才有东西可断。空闲时点了没有任何可见效果，人会以为没生效
  //   而反复点，等下一轮真跑起来时那几下反而把新任务打断了。
  // - 发消息：暂停中的 claude 是被 SIGSTOP 冻住的进程，输入只会堆在队列里 ——
  //   看着"已发送"，终端却毫无动静，是最容易让人以为「远程控制坏了」的一种。
  const canInterrupt = controllable && running;
  const canSend = controllable && !paused;
  // 禁用时把原因说出来，光是灰掉只会让人反复戳
  // 题面指纹：既做 SelectCard 的 key，也用来记「这道题我已经答过了」。
  //
  // 答完不等 hook 覆盖就先本地收起来：清除信号要等下一次工具调用（或 PostToolUse）
  // 才到，claude 在这中间若想久一点，一张已经答过的卡就一直杵在输入框上方占地方。
  // 答案本身会立刻作为 user 气泡出现在内容流里，反馈并不会丢。
  const pendingKey = React.useMemo(
    () => (task.pendingSelect ? JSON.stringify(task.pendingSelect) : ""),
    [task.pendingSelect],
  );
  const [answeredKey, setAnsweredKey] = useState("");
  // 必须有题才弹。改结构化之后没有了 JSON.parse 那道天然闸门：万一
  // AskUserQuestion 的 input 结构变了或给了个空壳，光判非空就会弹出一张
  // 什么都没有、还挡着输入框的卡片。
  const showPending =
    !!task.pendingSelect?.questions?.length && pendingKey !== answeredKey;

  const interruptHint = !controllable
    ? "该会话没有存活进程"
    : paused
      ? "已暂停，先恢复再中断"
      : running
        ? "中断当前任务"
        : "当前没有正在执行的任务";

  return (
    <div
      ref={rootRef}
      className={`${styles.ChatPane} ${compact ? styles.compact : ""} ${
        compact && showPending ? styles.awaitingSelect : ""
      }`}
      // 紧凑卡片整体可点：右侧那一列的用途就是「点它换到主区」，
      // 只让标题可点的话，卡片大半面积都是死的。头部按钮各自 stopPropagation。
      onClick={compact ? onActivate : undefined}
    >
      <header className={styles.paneHeader}>
        <div className={styles.headInfo}>
          {/* 状态放标题前；标题只显示会话标题（设备/IDE/PID 等杂项不再展示） */}
          <div className={styles.headTitle}>
            {/* 紧凑卡片里，「终端正等你选」必须显式标出来：卡片是只读的，
                选择卡本身不在这儿渲染，不给提示的话这个会话会一直干等着没人知道。
                点卡片换到主区即可作答。放在状态胶囊的位置 —— 此刻「在等你」
                比「执行中/等待输入」更该被先看到。 */}
            {compact && showPending ? (
              <span className={styles.pendingChip}>⌨ 待你选择</span>
            ) : (
              <span
                className={`${styles.statusChip} ${styles[task.status ?? ""] ?? ""}`}
              >
                {task.statusDsr}
              </span>
            )}
            {/* 号位：手机上看着这个号去钉钉发「@N …」。移动端头部是唯一能看到它的
                地方（侧栏是抽屉、看完就收起了），所以这里必须有。 */}
            {task.slot != null && (
              <Tooltip title={`钉钉里发「@${task.slot} 内容」即下发到这个终端`}>
                <span className={styles.slotChip}>@{task.slot}</span>
              </Tooltip>
            )}
            <span className={styles.headTitleText}>
              {task.title || task.prompt || task.projectName || "会话"}
            </span>
          </div>
          <div className={styles.headMeta}>
            {/* 拆分可同时看多设备的会话：标题下标明本会话所属设备 */}
            {task.hostname ? (
              <span className={styles.deviceChip}>💻 {task.hostname}</span>
            ) : null}
            <span>{task.projectName}</span>
            {task.usedTokens5h ? (
              <Tooltip title="近 5 小时 token 用量（输入 + 输出 + 缓存创建）">
                <span className={styles.tokenChip}>
                  5h · {fmtTokens(task.usedTokens5h)}
                </span>
              </Tooltip>
            ) : null}
          </div>
        </div>
        <div
          className={styles.headActions}
          // 紧凑卡片整卡可点（换到主区），但头部这些是各自独立的动作 ——
          // 不拦住冒泡的话，点「暂停」会连带把卡片换到主区。
          onClick={compact ? (e) => e.stopPropagation() : undefined}
        >
          {compact ? (
            /* 紧凑卡片只留「关闭」：卡片是拿来瞥一眼的，点它本体就换到主区，
               暂停/中断/终止这些都该在主区从容地做，摆在这儿既挤又容易误触。 */
            closable ? (
              <Tooltip title="关闭此格">
                <Button
                  size="small"
                  type="text"
                  icon={<CloseOutlined />}
                  onClick={() => closePane(id)}
                />
              </Tooltip>
            ) : null
          ) : narrow ? (
            // 收进下拉菜单的两种情形：放大布局右侧那一列的窄卡片，以及格子被切得太窄
            //（见 FLAT_MIN_W）。其余情况一律平铺 —— 功能藏在 ⋯ 里每次都要多点一下，
            // 而这些恰恰是高频操作。移动端不受这里影响：整个 paneHeader 被样式隐藏，
            // 操作走全局顶栏的「⋯」菜单。
            <Dropdown
              trigger={["click"]}
              placement="bottomRight"
              menu={{
                items: [
                  // 放大/还原排在最前：它是布局操作，比暂停这些更常用，
                  // 而多格时头部空间只够一个 ⋯，只能收进菜单（与其余按钮同一处境）。
                  {
                    key: "focus",
                    icon:
                      focusedId === id ? <CompressOutlined /> : <ExpandOutlined />,
                    label: focusedId === id ? "还原为网格" : "放大这一格",
                    onClick: () => setFocused(id),
                  },
                  {
                    key: "sync",
                    icon: <SyncOutlined />,
                    label: "重新同步内容",
                    onClick: () => syncMessages(id),
                  },
                  {
                    key: "pause",
                    icon: paused ? <PlayCircleOutlined /> : <PauseCircleOutlined />,
                    label: paused ? "恢复" : "暂停",
                    disabled: !controllable,
                    onClick: () => control(id, paused ? "resume" : "pause"),
                  },
                  {
                    key: "interrupt",
                    icon: <ThunderboltOutlined />,
                    // 菜单项写动作名，禁用的缘由挂 title —— 拿「当前没有正在执行的任务」
                    // 当条目名，读起来是句状态描述，不像个能点的东西
                    label: <span title={interruptHint}>中断当前任务</span>,
                    disabled: !canInterrupt,
                    onClick: () => control(id, "interrupt"),
                  },
                  {
                    key: "stop",
                    icon: <StopOutlined />,
                    label: "终止进程",
                    danger: true,
                    disabled: !controllable,
                    onClick: () =>
                      Modal.confirm({
                        title: "确定终止该任务进程？",
                        okText: "终止",
                        cancelText: "取消",
                        okButtonProps: { danger: true },
                        onOk: () => control(id, "stop"),
                      }),
                  },
                  { type: "divider" as const },
                  {
                    key: "close",
                    icon: <CloseOutlined />,
                    label: "关闭此格",
                    onClick: () => closePane(id),
                  },
                ],
              }}
            >
              <Button size="small" type="text" icon={<MoreOutlined />} />
            </Dropdown>
          ) : (
            <>
              {/* 放大/还原：单格没有意义（本来就占满），故与关闭按钮一样只在多格时出现 */}
              {closable ? (
                <Tooltip title={focusedId === id ? "还原为网格" : "放大这一格"}>
                  <Button
                    size="small"
                    type="text"
                    icon={
                      focusedId === id ? <CompressOutlined /> : <ExpandOutlined />
                    }
                    onClick={() => setFocused(id)}
                  />
                </Tooltip>
              ) : null}
              <Tooltip title="重新同步该终端的对话内容">
                <Button
                  size="small"
                  type="text"
                  icon={<SyncOutlined spin={loading} />}
                  onClick={() => syncMessages(id)}
                />
              </Tooltip>
              <Tooltip title={paused ? "恢复" : "暂停"}>
                <Button
                  size="small"
                  type="text"
                  icon={paused ? <PlayCircleOutlined /> : <PauseCircleOutlined />}
                  disabled={!controllable}
                  onClick={() => control(id, paused ? "resume" : "pause")}
                />
              </Tooltip>
              <Tooltip title={interruptHint}>
                {/* 禁用的 Button 不发事件，Tooltip 就没法解释「为什么不能点」，
                    所以包一层可悬停的 span */}
                <span>
                  <Button
                    size="small"
                    type="text"
                    icon={<ThunderboltOutlined />}
                    disabled={!canInterrupt}
                    onClick={() => control(id, "interrupt")}
                  />
                </span>
              </Tooltip>
              <Popconfirm
                title="确定终止该任务进程？"
                okText="终止"
                cancelText="取消"
                onConfirm={() => control(id, "stop")}
                disabled={!controllable}
              >
                <Tooltip title="终止进程">
                  <Button
                    size="small"
                    type="text"
                    danger
                    icon={<StopOutlined />}
                    disabled={!controllable}
                  />
                </Tooltip>
              </Popconfirm>
              {/* 关闭此格：只在多格时给 —— 单格关掉就空了，没有「回到网格」可言 */}
              {closable ? (
                <Tooltip title="关闭此格">
                  <Button
                    size="small"
                    type="text"
                    icon={<CloseOutlined />}
                    onClick={() => closePane(id)}
                  />
                </Tooltip>
              ) : null}
            </>
          )}
        </div>
      </header>

      <div className={styles.chat} ref={chatRef} onScroll={onChatScroll}>
        <Spin spinning={loading}>
          {!loading && feedMessages.length === 0 ? (
            <div className={styles.chatEmpty}>
              <div className={styles.big}>💬</div>
              <div>该会话暂无可展示的对话内容</div>
            </div>
          ) : (
            <div className={styles.chatColumn}>
              <TerminalFeed
                messages={feedMessages}
                running={task.status === "running"}
                providerDsr={task.providerDsr}
                imageCtx={imageCtx}
              />
            </div>
          )}
        </Spin>
      </div>

      {/* 清单与后台任务是「当前状态」而非时序事件：悬浮在本格右侧、可收起。
          紧凑卡片不给 —— 它只有 320×260，这组悬浮面板会盖掉大半内容，而卡片的用途
          就是「瞥一眼这个会话在干什么」。要看清单点一下把它换到主区即可。 */}
      {!compact && (
        <SessionPanels messages={messages} running={task.status === "running"} />
      )}

      {/* 排队条同样不进紧凑卡片：它整条都是操作（撤回、打断），而紧凑卡片是只读的 */}
      {!compact && queuedItems.length > 0 && (
        <div className={styles.queuedStrip}>
          <div className={styles.chatColumn}>
            <div className={styles.queuedHead}>
              <span className={styles.queuedTitle}>
                <span className={styles.queuedDot} />
                终端排队中 · {queuedItems.length}
              </span>
              <span className={styles.queuedActions}>
                <span
                  className={styles.recallAll}
                  role="button"
                  tabIndex={0}
                  onClick={() => {
                    const cmds = queuedItems
                      .filter((q) => q.recallable && q.cmdId)
                      .map((q) => q.cmdId as string);
                    const nativeCount = queuedItems.filter(
                      (q) => !q.recallable,
                    ).length;
                    // hub 队列里的（还没注入终端）走撤回 + 回填对话框
                    if (cmds.length) {
                      recallAllQueued(
                        id,
                        cmds,
                        queuedItems
                          .filter((q) => q.recallable)
                          .map((q) => q.text)
                          .join("\n"),
                      );
                    }
                    // 已进终端原生队列的，注入 ↑ 键逐条撤回（iTerm2/Windows）
                    if (nativeCount > 0) {
                      termKey(id, "up", nativeCount);
                    }
                  }}
                >
                  全部撤回
                </span>
                {/* 注入 Esc：打断终端当前正在跑的那一轮，排队的内容随即开始执行。
                    原先叫「插入会话」，看不出会打断什么 —— 而"打断"恰恰是这个按钮
                    最该让人先知道的后果。 */}
                <Tooltip title="打断终端当前正在执行的任务，让排队内容立即开始">
                  <span
                    className={styles.recallAll}
                    role="button"
                    tabIndex={0}
                    onClick={() => termKey(id, "esc")}
                  >
                    打断并执行
                  </span>
                </Tooltip>
              </span>
            </div>
            <div className={styles.queuedList}>
              {queuedItems.map((q, i) => (
                <div key={i} className={styles.queuedItem}>
                  <span className={styles.queuedItemText}>{q.text}</span>
                  {/* 单条撤回从对话流搬到这里 —— 撤回是「队列管理」，跟排队条同属一处；
                      留在正文里既与这块重复，又要为去重把消息藏起来。
                      已进终端原生队列的撤不回（只能整体注入 ↑），仍只给个标签。 */}
                  {q.recallable && q.cmdId ? (
                    <span
                      className={styles.queuedItemRecall}
                      role="button"
                      tabIndex={0}
                      onClick={() => recallInput(id, q.cmdId as string)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" || e.key === " ") {
                          e.preventDefault();
                          recallInput(id, q.cmdId as string);
                        }
                      }}
                    >
                      撤回
                    </span>
                  ) : (
                    <span className={styles.queuedTag}>已入终端队列</span>
                  )}
                </div>
              ))}
            </div>
            {queuedItems.some((q) => !q.recallable) ? (
              <div className={styles.queuedHint}>
                「全部撤回」注入 ↑、「打断并执行」注入 Esc（仅 iTerm2 / Windows）；
                Terminal.app 请在终端里手动按 ↑ / Esc（操作后此处自动同步）
              </div>
            ) : null}
          </div>
        </div>
      )}

      {/* 紧凑卡片不给输入区：卡片只有固定的一点高度，塞下输入框就没剩多少地方看内容。
          要发东西点一下把它换到主区 —— 那里才有完整的输入体验（附件、斜杠命令等）。 */}
      {compact ? null : (
      <div className={styles.composerWrap}>
        <div className={styles.chatColumn}>
          {/* 暂停中必须说破。SIGSTOP 冻住的进程从外面看就是「什么都不回」——
              而人在手机上只看到终端毫无动静，第一反应是远程控制坏了，不会想到
              是自己（或别人）点过暂停。所以把状态和出路一起摆在输入框正上方。 */}
          {paused ? (
            <div className={styles.pausedBar}>
              <span className={styles.pausedText}>
                该终端已暂停，发出去的内容不会被执行
              </span>
              <Button
                size="small"
                type="primary"
                icon={<PlayCircleOutlined />}
                onClick={() => control(id, "resume")}
              >
                恢复
              </Button>
            </div>
          ) : null}
          {/* 终端正等你选：由 hook 在选项弹出终端**之前**报上来，所以这里是「现在就能
              替它做决定」，而不是对话流里那张事后追认的记录卡。放在输入框正上方 ——
              人回到这个页面时视线本来就落在这儿，且它比打字更该被先处理。 */}
          {showPending && task.pendingSelect ? (
            <div className={styles.pendingSelect}>
              {/* key 必须跟着题目走：连着问两题时，React 会复用同一个 SelectCard
                  实例，它内部记「已回应」的 state 不会重置 —— 新题一弹出来就是
                  灰的锁定态，根本点不了。换 key 强制重挂载。 */}
              {/* 收起的时机是 onDone（所有题都答完），不是 onAnswer ——
                  AskUserQuestion 可以带多道题，答完第一题就把卡收了，
                  后面的题就再也没机会回答了。 */}
              <SelectCard
                key={pendingKey}
                data={task.pendingSelect}
                onAnswer={
                  canSend
                    ? (text) => sendInput(id, text, { fromSelect: true })
                    : undefined
                }
                onDone={() => setAnsweredKey(pendingKey)}
              />
            </div>
          ) : null}
          <Composer
            taskId={id}
            disabled={!canSend}
            disabledHint={
              paused ? "该终端已暂停，先恢复再发布" : "该会话无存活进程，无法发布"
            }
            machineId={task.machineId}
            // 会话此刻的工作目录优先：会话 cd 进子目录后，进程 cwd 还钉在启动目录，
            // 拿它当上传落点就会「文件写在项目根、终端在子目录里找」（见 Task.liveCwd）。
            cwd={task.liveCwd || task.process?.cwd}
            shellCwd={task.shellCwd}
            onSend={(text) => {
              sendInput(id, text);
              // 发送后强制滚到底部：即使之前上滚看历史，发出内容也应带着滚回底部
              stickBottomRef.current = true;
              requestAnimationFrame(() => {
                const el = chatRef.current;
                if (el) el.scrollTop = el.scrollHeight;
              });
            }}
          />
        </div>
      </div>
      )}
    </div>
  );
});

export default ChatPane;
