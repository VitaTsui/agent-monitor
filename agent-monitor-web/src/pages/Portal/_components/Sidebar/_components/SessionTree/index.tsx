import React, { useEffect, useState } from "react";

import { Tooltip } from "antd";
import {
  CaretDownOutlined,
  CaretRightOutlined,
  CheckCircleFilled,
  ClockCircleOutlined,
  CloseCircleFilled,
  DownOutlined,
  LaptopOutlined,
  LoadingOutlined,
  MinusCircleOutlined,
  PauseCircleFilled,
  SplitCellsOutlined,
} from "@ant-design/icons";
import { observer } from "mobx-react-lite";

import {
  PortalTaskData,
  SubTask,
  SubTaskOutcome,
} from "@/services/apis/portal";
import PortalStore from "../../../../PortalStore";
import {
  SUB_OUTCOME_LABEL,
  fmtSubTaskElapsed,
} from "../../../../_utils/sessionState";
import { sessionTitle } from "../../../../_utils/sessionNote";
import ScrollText from "../../../ScrollText";
import styles from "./index.module.scss";

const STATUS_LABEL: Record<string, string> = {
  running: "执行中",
  idle: "等待输入",
  paused: "已暂停",
  finished: "已结束",
};

/**
 * 会话状态的图标，与 VitaAgent 的任务状态是**同一套语言**
 * （`web/src/pages/chat/_components/TaskCard/index.tsx:80-86`）：
 * 运行中是转圈、等待是圈、终态是实心。
 *
 *   running  转圈（antd 自带 1s linear）＋ 主色  ← 对 `ph:circle-notch`
 *   idle     时钟圈、中性                        ← 对 `ph:circle-dashed`（等着人接话）
 *   paused   暂停圈、中性                        ← 对 `ph:minus-circle`（被按停，不是错）
 *   finished 实心勾、success                     ← 对 `ph:check-circle-fill`
 *
 * 原先这里是一枚 7×7 的彩色圆点，四态只靠颜色分（暂停还借了告警红 —— 按停不是
 * 出错）。执行链与智能体卡早就是这套字形图标了，侧栏再留一套色点，同一件事
 * 在一屏里就有两种画法。
 */
const STATUS_ICON: Record<string, React.ReactNode> = {
  running: <LoadingOutlined />,
  idle: <ClockCircleOutlined />,
  paused: <PauseCircleFilled />,
  finished: <CheckCircleFilled />,
};

/** 子代理的收场图标。与 `AgentCard` 的 `OUTCOME_ICON` 同一份字形，不另起一套 */
const OUTCOME_ICON: Record<SubTaskOutcome, React.ReactNode> = {
  running: <LoadingOutlined />,
  completed: <CheckCircleFilled />,
  failed: <CloseCircleFilled />,
  interrupted: <MinusCircleOutlined />,
};

/**
 * 收起了的客户端分组。**记「收起」而不是「展开」**，默认值就是全部展开 ——
 * 一个客户端下的会话本来就该看得见，折叠是用户主动要藏。
 */
const COLLAPSED_CLIENTS_KEY = "am.portal.sidebar.collapsedClients";
/**
 * 展开了的会话（子会话树）。这一边反过来记「展开」，**默认全部收起**：
 * 会话数量比客户端多一两个量级，默认全展开会把整列撑爆，而且每展开一条都要
 * 现读一次磁盘（见 PortalStore.loadSubTasks 的说明）。
 */
const EXPANDED_SESSIONS_KEY = "am.portal.sidebar.expandedSessions";

/**
 * 一条会话默认最多铺几个子代理。
 *
 * 实测单条会话能挂到 149 条 —— 全量铺在侧栏里，滚一屏都找不到主会话在哪。
 * 取最近的 20 条，其余收在一行「展开全部」后面。
 *
 * **纯渲染层截断**：清单本来就由 `/subtasks` 一次拉全并缓存在 store 里，
 * 展开全部不会再发请求。
 */
const SUBTASK_PREVIEW = 20;

/** 读一份 string[]；读不到（隐私模式 / 头一回 / 脏数据）就当空 */
const readIds = (key: string): string[] => {
  try {
    const raw = localStorage.getItem(key);
    const parsed = raw ? JSON.parse(raw) : null;
    return Array.isArray(parsed)
      ? parsed.filter((x) => typeof x === "string")
      : [];
  } catch {
    return [];
  }
};

const writeIds = (key: string, ids: string[]) => {
  try {
    localStorage.setItem(key, JSON.stringify(ids));
  } catch {
    // 隐私模式下写不进去也无妨，下次回到默认
  }
};

interface SessionTreeProps {
  isMobile: boolean;
  onSelect: (id: string) => void;
}

/**
 * 侧栏的会话树：**每个客户端一个可折叠分组，组里是它的全部会话，会话下面挂子代理**。
 *
 * 分工是定死的两件事，不许两处都能看内容：
 *
 *   - **左侧树 = 定位。** 一条会话派了谁、各自什么状态、跑了多久，在这里一眼看全。
 *   - **执行链 = 看内容。** 点左边那条子代理 → 打开父会话、滚到派出它的那张智能体卡
 *     并展开（见 `PortalStore.focusAgentCard` 与 `TerminalFeed` 的定位副作用）。
 *
 * 最早那一版把子代理当成一条只读会话列在这儿（复合 id `父::agentId`），
 * 三个毛病：子代理没有自己的进程/队列/备注/号位，塞进会话通道后每一处都要现场
 * 把这些字段清空；「谁派的、派在执行链的哪一步」在列表里丢掉了；打开之后正文与
 * 执行链里的智能体卡是同一份内容的两个入口。那套复合 id 已经整条删掉，
 * **这次恢复的是树，不是那条路由**。
 *
 * 这一块还推翻了更早的 `DeviceList` + `SessionList` 两段平铺结构。那套的三个毛病：
 *
 *   1. 设备是**单选**的，一次只看得到一台机器下的会话；
 *   2. 会话列表把「已结束且两小时没动」的整条滤掉 —— 于是历史会话永远看不到，
 *      看得到的只有正在回答的那几条（用户原话：「只会显示正在回答中的」）；
 *   3. 会话行只有标题和一个状态胶囊，**`lastAction` 一个字都没渲染**，
 *      所以「执行中」三个字之外看不到它到底在干什么。
 *
 * 交互照 VitaAgent 的项目树：**整行点击 = 打开这条会话，只有左边那个图标槽
 * 点击 = 展开/收起**。两个动作各有各的落点，不会互相抢。
 */
const SessionTree: React.FC<SessionTreeProps> = observer((props) => {
  const { isMobile, onSelect } = props;
  const {
    clientSections,
    openIds,
    splitOpen,
    keyword,
    subTasksOf,
    isSubTasksLoading,
    isSubTasksLoaded,
    isSubTasksPending,
    loadSubTasks,
    loadClientHistory,
  } = PortalStore;

  const [collapsedClients, setCollapsedClients] = useState<string[]>(() =>
    readIds(COLLAPSED_CLIENTS_KEY),
  );
  const [expandedSessions, setExpandedSessions] = useState<string[]>(() =>
    readIds(EXPANDED_SESSIONS_KEY),
  );
  /**
   * 子会话已经「展开全部」的那几条会话。
   *
   * **纯视图态，不落盘**：它只影响「当前这一眼怎么看」，子树收起再展开就该回到
   * 只显示 20 条 —— 持久化的话，一条 149 项的会话下次进来仍旧把侧栏撑爆。
   */
  const [fullSubs, setFullSubs] = useState<string[]>([]);
  /**
   * 把**已结束的子代理**也铺出来的那几条会话。
   *
   * 树回答的是「这条会话现在在发生什么」，所以默认只列还在跑的；跑完的收进
   * 最后一行。但**只是折叠，不是删除** —— 「回头找某个跑完的子代理」仍要有入口。
   *
   * 与 `fullSubs` 一样是**纯视图态，不落盘**：子树整条收起再展开就回到默认。
   */
  const [openDone, setOpenDone] = useState<string[]>([]);

  /* 这一列铺的就是 `selectedMachineId` 那台机器（见 PortalStore.clientSections），
     所以命令面板里「跳到某台设备」= 换这一列，不必再在这儿把那台机器的分组
     逐个展开。那段副作用连同它的理由一并删掉。 */
  /* 分组集合的指纹，只给下面那个副作用当依赖用。
     分隔符取 `,`：key 本身是 `machineId|provider`，`|` 不能用；`\0` 更不行 ——
     源码里夹一个 NUL 会让 grep 把整个文件判成二进制，从此谁都搜不到它。 */
  const sectionKeys = clientSections.map((s) => s.key).join(",");

  /* 展开着的分组就把第一页历史拉上。
     `loadClientHistory` 自己带幂等判断（拉过 + 关键字没变就直接返回），所以这里
     无脑调一遍即可；关键字变了会在 store 里作废旧页，这个副作用随即把新的拉回来。 */
  useEffect(() => {
    clientSections.forEach((sec) => {
      if (!collapsedClients.includes(sec.key)) {
        loadClientHistory(sec.key);
      }
    });
    // clientSections 是 computed，历史一回来它就换新引用；用 key 串当依赖，
    // 免得「拉回来 → 重渲染 → 又拉一次」空转
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sectionKeys, collapsedClients, keyword]);

  /* 刷新后仍然展开着的那些，得把子任务补回来 —— 展开态存了 localStorage，
     清单没存（它是服务端数据，存下来就会过期）。只在挂载时补一次。 */
  useEffect(() => {
    expandedSessions.forEach((id) => loadSubTasks(id));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /**
   * 子代理耗时要走字（「还在跑」与「卡死了」的唯一区别就是它在不在动）。
   * **只在真有子代理在跑时上表**，否则整棵树每秒白重渲染一次。
   */
  const ticking = expandedSessions.some((id) =>
    subTasksOf(id).some((t) => t.outcome === "running"),
  );
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!ticking) {
      return;
    }
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [ticking]);

  const toggleClient = (key: string) =>
    setCollapsedClients((prev) => {
      const next = prev.includes(key)
        ? prev.filter((k) => k !== key)
        : [...prev, key];
      writeIds(COLLAPSED_CLIENTS_KEY, next);
      return next;
    });

  const toggleSession = (id: string) => {
    const opening = !expandedSessions.includes(id);
    const next = opening
      ? [...expandedSessions, id]
      : expandedSessions.filter((x) => x !== id);
    setExpandedSessions(next);
    writeIds(EXPANDED_SESSIONS_KEY, next);
    // 子树一收起，「展开全部」与「已结束」两个视图态都跟着还原
    if (!opening) {
      setFullSubs((prev) => prev.filter((x) => x !== id));
      setOpenDone((prev) => prev.filter((x) => x !== id));
    }
    // **必须在 setState 的更新函数之外调**：那个函数跑在 React 的渲染阶段，
    // 在里面写 store 就是「渲染 A 组件时更新了 B 组件」，React 会直接报错。
    if (opening) {
      // 展开那一下才去读盘。收起再展开不重拉（清单缓存在 store 里）
      loadSubTasks(id);
    }
  };

  const toggleFullSubs = (id: string) =>
    setFullSubs((prev) =>
      prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id],
    );

  const toggleDone = (id: string) =>
    setOpenDone((prev) =>
      prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id],
    );

  if (clientSections.length === 0) {
    /* 两种成因，说法不同：**这台机器上一个客户端都没有**（换一台看得到别的），
       与**一台设备都没上报过**（该去接客户端了）。合成一句兜底的话，
       前者会被读成「系统坏了」。 */
    return (
      <div className={styles.emptyList}>
        {keyword
          ? `没有匹配「${keyword}」的会话`
          : PortalStore.deviceList.length > 0
            ? "这台设备上还没有任何终端会话。切换顶部的设备可以看别的机器。"
            : "还没有设备上报会话。请确认 agent-task-monitor 正在运行，且已在设备管理中信任。"}
      </div>
    );
  }

  /**
   * 一条子代理。**点它只做一件事：定位**（打开父会话 ＋ 滚到派出它的那张智能体卡）。
   *
   * 定位靠 `subTask.toolUseId` 与链上 `tool.id` 配对 —— 这是唯一的判据，
   * 不拿 label / 中文文案去凑（两边截断长度不同，必然错配）。拿不到 `toolUseId`
   * 的（老记录、起跑记录掉出重放窗口）**置灰不可点并说明原因**，
   * 不做成「点了没反应」——那是最让人反复戳的一种。
   */
  const renderSubTask = (parentId: string, st: SubTask) => {
    const locatable = !!st.toolUseId;
    /* 点过、但正文里确实没有它那一步。**原因说在这一行上** ——
       不弹飘过去的全局提示（得让人回头找刚点的是哪条），也不装作没事发生。 */
    const missed = PortalStore.focusMissId === st.id;
    const label = SUB_OUTCOME_LABEL[st.outcome] ?? st.status;
    const elapsed = fmtSubTaskElapsed(st, now);
    const hint = !locatable
      ? "这个子代理没有留下起跑记录（tool_use_id），在执行链上定位不到它"
      : missed
        ? "派出它的那次工具调用不在已加载的正文里，执行链上定位不到那一步"
        : `${st.label} · ${label}`;
    const go = () => {
      onSelect(parentId);
      PortalStore.focusAgentCard(parentId, st.id);
    };
    return (
      <div
        key={st.id}
        className={`${styles.item} ${styles.subItem} ${
          locatable ? "" : styles.itemDisabled
        }`}
        role={locatable ? "button" : undefined}
        tabIndex={locatable ? 0 : undefined}
        aria-disabled={!locatable}
        title={hint}
        onClick={locatable ? go : undefined}
        onKeyDown={
          locatable
            ? (e) => {
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  go();
                }
              }
            : undefined
        }
      >
        {/* 收场**只看 outcome**，一个上游字面量都不匹配：`killed` 是父会话被中断时
            一次性发给所有在跑子代理的统一通知，按它配色就会「按一下 Esc 一排全爆红」。
            `interrupted` 因此走中性的减号圈，不是失败的叉。 */}
        <span
          className={`${styles.leadSlot} ${styles.subIcon} ${
            styles[st.outcome] ?? ""
          }`}
          aria-label={label}
        >
          {OUTCOME_ICON[st.outcome]}
        </span>
        <span className={styles.itemTitle}>{st.label}</span>
        {/* 定位失败就把话说在这儿，替掉耗时那一格（那一眼要的是「为什么没反应」） */}
        {missed ? (
          <span className={styles.subMiss}>定位不到</span>
        ) : elapsed ? (
          <span className={styles.subMeta}>{elapsed}</span>
        ) : null}
      </div>
    );
  };

  /** 一条会话（主会话行 ＋ 展开后的子代理） */
  const renderSession = (t: PortalTaskData) => {
    const id = t.id ?? "";
    const active = openIds.includes(id);
    const status = t.status ?? "";
    const statusLabel = STATUS_LABEL[status] ?? t.statusDsr ?? "";
    /* 树里**只列子代理**：后台命令在执行链上根本没有节点，列出来只能是一排
       永远灰着的行；它们该看的地方是右栏「会话状态」里的后台任务卡。 */
    const subs = subTasksOf(id).filter((st) => st.kind === "agent");
    const expanded = expandedSessions.includes(id);
    const loadingSubs = isSubTasksLoading(id);
    const pendingSubs = isSubTasksPending(id);
    /* 给不给展开箭头：拉过且一条没有 → 确定没有子代理，不给（给了点下去什么都不会出现）；
       还没拉过 → 给，点了才去问。活跃会话手里已经有一份 `Task.subTasks`，
       但那份只覆盖近 24 小时 / 50 条，空不代表真的没有。 */
    const expandable = subs.length > 0 || !isSubTasksLoaded(id);
    /**
     * **执行中的会话第二行显示它此刻在干什么**（`正在调用工具: Bash` 这类）。
     *
     * 这条数据后端一直在下发（`PortalTaskData.lastAction`），但全前端一处都没渲染 ——
     * 于是「执行中」永远只有那三个字。用户为此提了三次。
     */
    const lastAction = status === "running" ? (t.lastAction ?? "").trim() : "";
    /** 展开/收起的箭头字形。悬停替换那枚与触屏常驻那枚是同一个，不另起一套 */
    const caretIcon = expanded ? <CaretDownOutlined /> : <CaretRightOutlined />;
    /**
     * **默认只列还在跑的**。跑完的收进最后一行「已结束 N 个」，点开才铺。
     *
     * 侧栏树回答的是「这条会话现在在发生什么」——一屏全是跑完的子代理，
     * 等于一屏没有信息（用户原话：「完成了的子会话，为什么还会显示」）。
     * 收起来而不是删掉：回头找某个跑完的子代理仍然要有入口。
     *
     * **执行链不受这条影响**：链要的是完整的「它派过谁」，一条都不能少
     * （那条「只留未完成」的过滤早就撤销过，不要在这儿借尸还魂）。
     */
    const running = subs.filter((st) => st.outcome === "running");
    const done = subs.filter((st) => st.outcome !== "running");
    /** 跑砸的那几个单独报一笔：把失败并进「已结束」不算说谎，但也不该被淹掉 */
    const failedCount = done.filter((st) => st.outcome === "failed").length;
    const doneOpen = openDone.includes(id);
    /* **过滤决定列表里有谁，截断决定铺出来几条**，两件事分开、互不打架：
       未完成的自己就超过 20 条时（实测单条会话挂到过 149 个子代理），
       截断照样在这份列表上生效，「展开全部」仍然只是渲染层的事、不发请求。 */
    const visible = doneOpen ? subs : running;
    /* 默认只铺最近 20 条。后端给的是时间顺序（旧 → 新），所以「最近的那一端」
       是数组末尾 —— 用 slice(-N) 取，顺序保持不变。 */
    const showAll = fullSubs.includes(id);
    const shownSubs =
      showAll || visible.length <= SUBTASK_PREVIEW
        ? visible
        : visible.slice(-SUBTASK_PREVIEW);

    return (
      <div key={id}>
        <div
          className={`${styles.item} ${styles.sessionItem} ${
            active ? styles.itemActive : ""
          } ${lastAction ? styles.itemTall : ""}`}
          role="button"
          tabIndex={0}
          aria-current={active}
          onClick={() => onSelect(id)}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              onSelect(id);
            }
          }}
        >
          {/* 行首那一格：**静止是状态图标，鼠标移到这一行上就换成展开箭头**
              （照 VitaAgent 的侧栏，`Sidebar/index.module.scss:790-810` 同一手法）。
              两个记号叠在**同一个 20×20 的槽**里，靠 CSS 切显隐 —— 不各占一列，
              侧栏就这么宽，一列记号能换来一列文字。

              **不能展开的行不换箭头**：换出来点了没反应，那是骗人。

              触屏没有 hover，纯悬浮替换等于没有展开入口。所以另有一个
              `.caret` 列：桌面端宽度是 0（等于不存在），`@media (hover: none)`
              下才撑到 14px、把箭头常驻出来 —— 状态图标仍留在槽里，不被挤掉。
              两套的缩进、导引线都由同一个 `--caret-col` 推出来（见 scss）。

              点击分工：点这一格 = 展开/收起（`stopPropagation` 掐掉冒泡），
              点行的其余部分 = 打开会话。

              状态不再另印一个文字胶囊：图标已经把四态说清楚了，语义由
              `aria-label` / `title` 承担（见「去掉冗余文字」那条）。 */}
          <span
            className={`${styles.lead} ${expandable ? styles.leadToggle : ""}`}
            role={expandable ? "button" : undefined}
            tabIndex={expandable ? -1 : undefined}
            aria-expanded={expandable ? expanded : undefined}
            aria-label={
              expandable
                ? `${statusLabel} · ${expanded ? "收起子代理" : "展开子代理"}`
                : statusLabel
            }
            title={
              expandable
                ? `${statusLabel} · ${expanded ? "收起子代理" : "展开子代理"}`
                : statusLabel
            }
            onClick={
              expandable
                ? (e) => {
                    e.stopPropagation();
                    toggleSession(id);
                  }
                : undefined
            }
          >
            {/* 触屏专用的常驻箭头列。桌面端 width: 0，只剩一个不占地方的空壳；
                不可展开的行同样留这个空壳，触屏下所有会话行的文字才从同一个 x 起 */}
            <span className={styles.caret} aria-hidden>
              {expandable ? caretIcon : null}
            </span>
            <span
              className={`${styles.leadSlot} ${styles.statusIcon} ${
                styles[status] ?? ""
              }`}
            >
              <span className={styles.leadRest}>{STATUS_ICON[status]}</span>
              {/* 悬停时顶替状态图标的那枚箭头。只有能展开的行才有 */}
              {expandable ? (
                <span className={styles.leadHover} aria-hidden>
                  {caretIcon}
                </span>
              ) : null}
            </span>
          </span>

          <div className={styles.sessBody}>
            <div className={styles.sessRow}>
              <ScrollText
                className={styles.itemTitle}
                active={active}
                plain={sessionTitle(t, "新会话")}
                text={sessionTitle(t, "新会话")}
              />
              {/* 号位：与钉钉「#N」同一个编号，在手机上照着这个号下发。
                  **放在标题行的末尾，不放在标题前面** —— 它从前插在图标槽与标题
                  之间，把会话行的文字列往右推了 26px（徽标 18 ＋ gap 8），比子行
                  30px 缩进带来的 20px 还多。于是「父的字比子的字更靠右」，一列扫
                  下去父子完全分不出层级（用户原话：「子会话，为什么是都堆在和主
                  会话同一层级」）。而且这 26px 只有**带号位的会话**才有 ——
                  同一棵树里，有号位的看着是平的、没号位的又是缩进的。
                  现在文字列上只剩 `[缩进][图标槽 20][标题]`，每一行都一样，
                  父子差就恒等于那 20px 的缩进。 */}
              {t.slot != null && (
                <Tooltip title={`钉钉里发「#${t.slot} 内容」即下发到这个终端`}>
                  <span className={styles.sessSlot}>{t.slot}</span>
                </Tooltip>
              )}
            </div>
            {/* 正在干什么。单行截断 —— 它是一眼扫过去的补充信息，
                不该把一行会话撑成三行 */}
            {lastAction ? (
              <div className={styles.sessAction} title={lastAction}>
                {lastAction}
              </div>
            ) : null}
          </div>

          {/* 移动端窄屏不支持拆分并排，去掉拆分按钮，只单会话查看 */}
          {!isMobile && (
            <Tooltip title="拆分显示">
              <SplitCellsOutlined
                className={styles.splitBtn}
                onClick={(e) => {
                  e.stopPropagation();
                  splitOpen(id);
                }}
              />
            </Tooltip>
          )}
        </div>

        {expanded ? (
          <div className={styles.subBody}>
            {loadingSubs && !subs.length ? (
              <div
                className={`${styles.item} ${styles.subItem} ${styles.subHint}`}
              >
                <span className={styles.leadSlot}>
                  <LoadingOutlined />
                </span>
                <span className={styles.itemTitle}>正在读取子代理…</span>
              </div>
            ) : pendingSubs && !subs.length ? (
              /* `pending: true` **不是空**：那台机器还没把清单送回来。
                 说成「没有子代理」是一句假话，还会把人支去终端里翻。 */
              <div
                className={`${styles.item} ${styles.subItem} ${styles.subHint}`}
              >
                <span className={styles.itemTitle}>
                  读取中：这台机器还没把子代理清单送回来
                </span>
                <span
                  className={styles.subRetry}
                  role="button"
                  tabIndex={0}
                  onClick={(e) => {
                    e.stopPropagation();
                    loadSubTasks(id, true);
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      loadSubTasks(id, true);
                    }
                  }}
                >
                  重试
                </span>
              </div>
            ) : subs.length ? (
              <>
                {shownSubs.map((st) => renderSubTask(id, st))}
                {/* 截断提示行。样式与「查看全部会话」同一档（32 高 / 14 / muted）：
                    它是当前这份列表的最后一行，不是一颗按钮。
                    **纯渲染层截断，不发请求** —— 清单早就一次拉全缓存在 store 里了。
                    数的是 `visible`（当前列表）不是 `subs`（全部）：折叠着已结束的
                    时候写「共 25 条」而眼前只有 3 条在跑，那句话就对不上眼前的列表。 */}
                {visible.length > SUBTASK_PREVIEW ? (
                  <div
                    className={styles.subMore}
                    role="button"
                    tabIndex={0}
                    onClick={() => toggleFullSubs(id)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" || e.key === " ") {
                        e.preventDefault();
                        toggleFullSubs(id);
                      }
                    }}
                  >
                    {showAll ? "收起" : `展开全部（共 ${visible.length} 条）`}
                  </div>
                ) : null}
                {/* 已结束的那些收在这一行后面。
                    **文案不说「已完成」** —— 失败与被中断的也在这堆里，
                    管它们叫完成就是说假话。「已结束」对三种收场都成立；
                    真有跑砸的就单报一笔，别让它被这行字淹掉。 */}
                {done.length ? (
                  <div
                    className={styles.subMore}
                    role="button"
                    tabIndex={0}
                    aria-expanded={doneOpen}
                    onClick={() => toggleDone(id)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" || e.key === " ") {
                        e.preventDefault();
                        toggleDone(id);
                      }
                    }}
                  >
                    {doneOpen ? (
                      `收起已结束的 ${done.length} 个`
                    ) : (
                      <>
                        {`已结束 ${done.length} 个`}
                        {failedCount ? (
                          <span className={styles.subMoreBad}>
                            {`${failedCount} 个失败`}
                          </span>
                        ) : null}
                      </>
                    )}
                  </div>
                ) : null}
              </>
            ) : (
              <div
                className={`${styles.item} ${styles.subItem} ${styles.subHint}`}
              >
                <span className={styles.itemTitle}>该会话没有派过子代理</span>
              </div>
            )}
          </div>
        ) : null}
      </div>
    );
  };

  return (
    <div className={styles.SessionTree}>
      {clientSections.map((sec) => {
        const collapsed = collapsedClients.includes(sec.key);
        return (
          <div key={sec.key} className={styles.group}>
            <div className={styles.groupHead}>
              <button
                type="button"
                className={styles.groupBtn}
                aria-expanded={!collapsed}
                onClick={() => toggleClient(sec.key)}
              >
                <LaptopOutlined className={styles.groupIcon} />
                {/* **组名只写客户端名**（`Claude Code` / `Codex` / `ChatGPT 桌面版`）。
                    主机名与「本机」徽标都搬到了顶部的设备选择器上 ——
                    这一列铺的就是那台机器的会话，每一行组标题再重复一遍机器名
                    纯属占地方，机器一多还会让同一个客户端名出现好几遍。 */}
                <span className={styles.groupLabel}>{sec.providerDsr}</span>
                <DownOutlined
                  className={`${styles.groupCaret} ${
                    collapsed ? styles.groupCaretUp : ""
                  }`}
                />
              </button>
              {/* 这台客户端此刻有几条在跑。收起时这是唯一还看得见的动静 */}
              {sec.running > 0 ? (
                <span className={styles.groupRunning}>
                  {sec.running} 执行中
                </span>
              ) : null}
            </div>

            {!collapsed ? (
              <div className={styles.groupBody}>
                {sec.buckets.map((b) => (
                  <div key={b.label}>
                    <div className={styles.subLabel}>{b.label}</div>
                    {b.items.map(renderSession)}
                  </div>
                ))}

                {/* 还没拉回历史时说一声，别让人以为这台机器只有这几条 */}
                {sec.loading && !sec.buckets.length ? (
                  <div className={styles.groupHint}>
                    <LoadingOutlined /> 正在读取历史会话…
                  </div>
                ) : null}
                {!sec.loading && !sec.buckets.length ? (
                  <div className={styles.groupHint}>
                    {keyword ? "没有匹配的会话" : "这个客户端还没有会话"}
                  </div>
                ) : null}

                {/* 翻页用时间游标（nextCursor），不是页码 —— 列表底料随上报刷新，
                    用 offset 会让某条被跳过或看两遍 */}
                {sec.hasMore ? (
                  <div
                    className={styles.loadMore}
                    role="button"
                    tabIndex={0}
                    onClick={() => loadClientHistory(sec.key, true)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" || e.key === " ") {
                        e.preventDefault();
                        loadClientHistory(sec.key, true);
                      }
                    }}
                  >
                    {sec.loading ? (
                      <>
                        <LoadingOutlined /> 加载中…
                      </>
                    ) : (
                      `加载更早的会话（共 ${sec.total} 条）`
                    )}
                  </div>
                ) : null}
              </div>
            ) : null}
          </div>
        );
      })}
    </div>
  );
});

export default SessionTree;
