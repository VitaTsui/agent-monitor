import React, { useEffect, useState } from "react";

import { Tooltip } from "antd";
import { Icon } from "@hsu-react/ui";
import { observer } from "mobx-react-lite";

import { PortalTaskData } from "@/services/apis/portal";
import PortalStore from "../../../../PortalStore";
import { sessionTitle } from "../../../../_utils/sessionNote";
import ScrollText from "../../../ScrollText";
import StatusIcon, { statusOfSession } from "../../../StatusIcon";
import styles from "./index.module.scss";

const STATUS_LABEL: Record<string, string> = {
  running: "执行中",
  idle: "等待输入",
  paused: "已暂停",
  finished: "已结束",
};

/*
 * 状态图标**不在这儿定义**。四处（侧栏会话行 / 侧栏子代理行 / 执行链 / 智能体卡）
 * 共用 `_components/StatusIcon`：那一份是 Phosphor 本尊（`ph:circle-notch` 等），
 * 颜色与转圈由它自带，调用方只负责给尺寸（写在槽的 `font-size` 上）。
 *
 * 从前这里自带一份 antd 映射（`LoadingOutlined` / `CheckCircleFilled` …）——
 * 尺寸颜色对齐过三轮，形状始终对不上，两套图标集的字形本来就不同；一屏里同时
 * 出现 antd 的细弧线和 Phosphor 的缺口圆，一眼看得出是两套。整份删掉，不留并存。
 */

/**
 * 收起了的客户端分组。**记「收起」而不是「展开」**，默认值就是全部展开 ——
 * 一个客户端下的会话本来就该看得见，折叠是用户主动要藏。
 */
const COLLAPSED_CLIENTS_KEY = "am.portal.sidebar.collapsedClients";
/**
 * 收起了的项目。**与会话的展开态分开存**：两者数量级差一个量级、默认值也相反，
 * 混进一个键里就没法各自表达默认态（项目默认展开 = 记「收起」，
 * 会话默认收起 = 记「展开」）。
 */
const COLLAPSED_PROJECTS_KEY = "am.portal.sidebar.collapsedProjects";

/**
 * 子会话树撤掉后留在用户浏览器里的那个键。**主动删掉**，不是放着不管 ——
 * 没有任何代码再读它，留着就是一份谁也说不清来历的脏数据。
 * 与 `PortalStore` 里 `RIGHT_PANE_LEGACY_KEYS` 那次清理同一个做法。
 */
const LEGACY_KEYS = ["am.portal.sidebar.expandedSessions"];
try {
  LEGACY_KEYS.forEach((k) => localStorage.removeItem(k));
} catch {
  // 隐私模式下删不掉也无妨：那儿本来也存不下东西
}

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
 * 侧栏的会话树：**客户端组 → 项目 → 会话**，到此为止。
 *
 * **子代理不在这儿。** 它只在执行链里看（正文里的智能体卡，见 `AgentCard`）——
 * 一份内容不留两个入口。这一侧曾经有过两版都被撤掉了：
 *
 *   1. 把子代理当成一条只读会话列在这儿（复合 id `父::agentId`）。子代理没有自己的
 *      进程/队列/备注/号位，塞进会话通道后每一处都要现场把这些字段清空。
 *   2. 挂成第四级的子会话树，点一条就滚到执行链上对应的那张卡。树是能看了，
 *      但一条会话展开出几十项之后，侧栏回答不了「我在哪个项目上开着哪几个终端」
 *      这个它本来该回答的问题。
 *
 * 现在这一列只回答那一个问题，四级收成三级。
 *
 * 这一块还推翻了更早的 `DeviceList` + `SessionList` 两段平铺结构。那套的三个毛病：
 *
 *   1. 设备是**单选**的，一次只看得到一台机器下的会话；
 *   2. 会话列表把「已结束且两小时没动」的整条滤掉 —— 于是历史会话永远看不到，
 *      看得到的只有正在回答的那几条（用户原话：「只会显示正在回答中的」）；
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
    loadClientHistory,
  } = PortalStore;

  const [collapsedClients, setCollapsedClients] = useState<string[]>(() =>
    readIds(COLLAPSED_CLIENTS_KEY),
  );
  /**
   * 收起了的项目。**记「收起」而不是「展开」，默认全展开** ——
   * CLI 那一列只列当前打开的终端，通常就几条、分布在一两个项目上；默认收起等于
   * 把用户明明开着的东西全藏起来，进来还得先点两下。与客户端分组同一个思路。
   */
  const [collapsedProjects, setCollapsedProjects] = useState<string[]>(() =>
    readIds(COLLAPSED_PROJECTS_KEY),
  );
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

  const toggleClient = (key: string) =>
    setCollapsedClients((prev) => {
      const next = prev.includes(key)
        ? prev.filter((k) => k !== key)
        : [...prev, key];
      writeIds(COLLAPSED_CLIENTS_KEY, next);
      return next;
    });

  const toggleProject = (key: string) =>
    setCollapsedProjects((prev) => {
      const next = prev.includes(key)
        ? prev.filter((k) => k !== key)
        : [...prev, key];
      writeIds(COLLAPSED_PROJECTS_KEY, next);
      return next;
    });

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
   * 一条会话。**到此为止，下面不再挂子代理。**
   *
   * 子代理只在执行链里看（正文里的智能体卡，见 `AgentCard`）。侧栏这一侧的子会话树
   * 连同它那一整套（展开箭头与展开态持久化、20 条截断的「展开全部」、「已结束 N 个」
   * 折叠、读取中/重试提示、那条导引线与第四级缩进）整个撤掉 —— 侧栏回答的是
   * 「我在哪个项目上开着哪几个终端」，一条会话展开出几十项子代理之后滚一屏都找不到
   * 主会话在哪；而那些内容执行链上本来就有，还带着上下文（谁在哪一步派的）。
   * 同一份东西不留两个入口。
   */
  const renderSession = (t: PortalTaskData) => {
    const id = t.id ?? "";
    const active = openIds.includes(id);
    const status = t.status ?? "";
    const statusLabel = STATUS_LABEL[status] ?? t.statusDsr ?? "";
    return (
      <div
        key={id}
        className={`${styles.item} ${styles.sessionItem} ${
          active ? styles.itemActive : ""
        }`}
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
        {/* 行首只剩状态图标。**没有展开箭头、也没有悬浮替换** —— 会话行底下已经
            没有可展开的东西，换出一个箭头点下去什么都不会发生，那是骗人。
            （项目行仍然保留那一套：它底下还有会话。） */}
        <span
          className={styles.leadSlot}
          aria-label={statusLabel}
          title={statusLabel}
        >
          <StatusIcon kind={statusOfSession(status)} />
        </span>

        {/* 只剩标题一行。**第二行那个 `lastAction`（`正在调用工具: Bash…`）撤掉了** ——
            它是「侧栏看不出在执行什么」那一版的补丁；现在右缘的徽章已经全清、层级
            也收成三级，侧栏回答的是「我在哪个项目上开着哪几个终端」，而「此刻在干
            什么」执行链上写得更清楚、还带着上下文。一条信息不摆两处。
            `PortalTaskData.lastAction` 这个字段没动，后端照常产出，只是侧栏不读它。

            外面那两层 `.sessBody` / `.sessRow` 也一并去掉：它们当初是为「标题 ＋ 第二行」
            与「标题 ＋ 状态胶囊」这两种并排才存在的，现在里面只剩一个标题，
            留着就是两层什么都不做的盒子（`.itemTitle` 自己带 flex:1 / min-width:0）。 */}
        <ScrollText
          className={styles.itemTitle}
          active={active}
          plain={sessionTitle(t, "新会话")}
          text={sessionTitle(t, "新会话")}
        />

        {/* 移动端窄屏不支持拆分并排，去掉拆分按钮，只单会话查看 */}
        {!isMobile && (
          <Tooltip title="拆分显示">
            <Icon
              icon="ph:columns"
              className={styles.splitBtn}
              onClick={(e) => {
                e.stopPropagation();
                splitOpen(id);
              }}
            />
          </Tooltip>
        )}
      </div>
    );
  };

  /**
   * CLI 分组里的一个**项目**。
   *
   * **整行点击 = 展开/收起**，没有第二种含义 —— 本项目没有「项目页」这个东西
   * （参照 VitaAgent 的项目行整行点击是进项目页、只有图标槽点击才展开，那一套在
   * 这儿没有落点）。所以行首那一格不再单独接点击，免得同一个动作有两个落点。
   *
   * 图标槽与会话行共用同一副机制：静止显文件夹、悬停换箭头（见 `.lead` / `.caret`）。
   */
  const renderProject = (
    sec: { key: string },
    proj: { key: string; title: string; items: PortalTaskData[] },
  ) => {
    const pkey = `${sec.key}::${proj.key}`;
    const open = !collapsedProjects.includes(pkey);
    return (
      <div key={pkey}>
        <div
          className={`${styles.item} ${styles.projectItem}`}
          role="button"
          tabIndex={0}
          aria-expanded={open}
          title={`${proj.title} · ${proj.items.length} 个终端`}
          onClick={() => toggleProject(pkey)}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              toggleProject(pkey);
            }
          }}
        >
          <span className={`${styles.lead} ${styles.leadToggle}`}>
            <span className={styles.caret} aria-hidden>
              {open ? (
                <Icon icon="ph:caret-down" />
              ) : (
                <Icon icon="ph:caret-right" />
              )}
            </span>
            <span className={`${styles.leadSlot} ${styles.projectIcon}`}>
              {/* 文件夹取 Phosphor 的 `ph:folder-simple`，与旁边那一列状态图标同一套
                  笔画 —— antd 的 `FolderOutlined` 比它细一档，并排能看出是两套。

                  **开合两态用同一枚字形**，照参照（VitaAgent `Sidebar/index.tsx:1024-1034`
                  实测：静止恒为 `ph:folder-simple`，只有 hover 才换 caret）。
                  从前是 `FolderOutlined` / `FolderOpenOutlined` 对着切，两个文件夹
                  的轮廓本来就不一样，一开一合等于图标在跳；而「开没开」这件事
                  底下有没有铺出东西已经说得很清楚了，hover 上来还有 caret。

                  **不塞进 `StatusIcon`**：那个组件按 `StatusKind` 配色配动画，
                  文件夹不是状态。按项目既有约定直接用 `@hsu-react/ui` 的 `Icon`
                  （`StatusIcon` 内部用的也是它），不新起第二套图标组件。 */}
              <span className={styles.leadRest}>
                <Icon icon="ph:folder-simple" />
              </span>
              <span className={styles.leadHover} aria-hidden>
                {open ? (
                <Icon icon="ph:caret-down" />
              ) : (
                <Icon icon="ph:caret-right" />
              )}
              </span>
            </span>
          </span>
          <span className={styles.itemTitle}>{proj.title}</span>
        </div>
        {open ? (
          <div className={styles.projectBody}>
            {proj.items.map(renderSession)}
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
                <Icon icon="ph:laptop" className={styles.groupIcon} />
                {/* **组名只写客户端名**（`Claude Code` / `Codex` / `ChatGPT 桌面版`）。
                    主机名与「本机」徽标都搬到了顶部的设备选择器上 ——
                    这一列铺的就是那台机器的会话，每一行组标题再重复一遍机器名
                    纯属占地方，机器一多还会让同一个客户端名出现好几遍。 */}
                <span className={styles.groupLabel}>{sec.providerDsr}</span>
                {/* 折叠箭头换 Phosphor。**尺寸本来就是 12px**（与参照
                    `Sidebar/index.module.scss:659-663` 实测一致），看着大是因为
                    antd 的 `DownOutlined` 字形把 em 框填得更满 —— 同样 12px，
                    它画出来的实体比 `ph:caret-down` 大一圈。和文件夹那次是同一类
                    问题：不是把数字调小，是把字形换成同一套。 */}
                <Icon
                  icon="ph:caret-down"
                  className={`${styles.groupCaret} ${
                    collapsed ? styles.groupCaretUp : ""
                  }`}
                />
              </button>
            </div>

            {!collapsed ? (
              <div className={styles.groupBody}>
                {/* **CLI 走项目、桌面走时间桶，二选一不并存。**
                    CLI 那一列全是开着的终端窗口，问的是「我在哪个项目上开着哪几个」；
                    桌面那一列是可回溯的对话历史，问的是「那是什么时候的」。
                    桌面客户端的对话没有 cwd，硬塞一层「未知项目」是凭空多一级缩进。 */}
                {sec.desktop
                  ? sec.buckets.map((b) => (
                      <div key={b.label}>
                        <div className={styles.subLabel}>{b.label}</div>
                        {b.items.map(renderSession)}
                      </div>
                    ))
                  : sec.projects.map((proj) => renderProject(sec, proj))}

                {/* 还没拉回历史时说一声，别让人以为这台机器只有这几条 */}
                {sec.loading && !sec.buckets.length && !sec.projects.length ? (
                  <div className={styles.groupHint}>
                    <StatusIcon kind="running" plain /> 正在读取历史会话…
                  </div>
                ) : null}
                {!sec.loading && !sec.buckets.length && !sec.projects.length ? (
                  <div className={styles.groupHint}>
                    {keyword
                      ? "没有匹配的会话"
                      : sec.desktop
                        ? "这个客户端还没有会话"
                        : /* CLI 空着**不等于**它没跑过东西 —— 那台机器上多半攒了
                             一堆 jsonl，只是此刻一个终端都没开着。说成「还没有
                             会话」是假话。 */
                          "当前没有打开的终端会话"}
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
                        <StatusIcon kind="running" plain /> 加载中…
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
