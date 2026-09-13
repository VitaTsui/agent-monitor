import React, { useEffect, useState } from "react";

import { Tooltip } from "antd";
import {
  DownOutlined,
  LaptopOutlined,
  LoadingOutlined,
  SplitCellsOutlined,
} from "@ant-design/icons";
import { observer } from "mobx-react-lite";

import { PortalTaskData } from "@/services/apis/portal";
import PortalStore from "../../../../PortalStore";
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
 * 收起了的客户端分组。**记「收起」而不是「展开」**，默认值就是全部展开 ——
 * 一个客户端下的会话本来就该看得见，折叠是用户主动要藏。
 */
const COLLAPSED_CLIENTS_KEY = "am.portal.sidebar.collapsedClients";
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
 * 侧栏的会话树：**每个客户端一个可折叠分组，组里是它的全部会话**。
 *
 * 会话下面**不再挂子会话**。那一版把子代理当成一条只读会话列在这儿（复合 id
 * `父::agentId`），三个毛病：子代理没有自己的进程/队列/备注/号位，塞进会话通道后
 * 每一处都要现场把这些字段清空；「谁派的、派在执行链的哪一步」在列表里丢掉了；
 * 而侧栏本来要回答的是「我最近在弄什么」，一条会话展开 149 项子代理之后，
 * 滚一屏都找不到主会话。子代理现在就地画在正文的执行链上（见 `AgentCard`）。
 *
 * 这一块推翻了原来的 `DeviceList` + `SessionList` 两段平铺结构。那套的三个毛病：
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
    loadClientHistory,
  } = PortalStore;

  const [collapsedClients, setCollapsedClients] = useState<string[]>(() =>
    readIds(COLLAPSED_CLIENTS_KEY),
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

  /** 一条会话 */
  const renderSession = (t: PortalTaskData) => {
    const id = t.id ?? "";
    const active = openIds.includes(id);
    const status = t.status ?? "";
    /**
     * **执行中的会话第二行显示它此刻在干什么**（`正在调用工具: Bash` 这类）。
     *
     * 这条数据后端一直在下发（`PortalTaskData.lastAction`），但全前端一处都没渲染 ——
     * 于是「执行中」永远只有那三个字。用户为此提了三次。
     */
    const lastAction = status === "running" ? (t.lastAction ?? "").trim() : "";

    return (
      <div
        key={id}
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
        {/* 图标槽：会话状态点。
            子会话树撤掉之后这里没有可展开的东西了 —— 点它不再有第二种含义，
            整行只有一个动作：打开这条会话 */}
        <span className={styles.leadSlot}>
          <span className={`${styles.dot} ${styles[status] ?? ""}`} />
        </span>
        {/* 号位：与钉钉「#N」同一个编号，在手机上照着这个号下发 */}
        {t.slot != null && (
          <Tooltip title={`钉钉里发「#${t.slot} 内容」即下发到这个终端`}>
            <span className={styles.sessSlot}>{t.slot}</span>
          </Tooltip>
        )}

        <div className={styles.sessBody}>
          <div className={styles.sessRow}>
            <ScrollText
              className={styles.itemTitle}
              active={active}
              plain={sessionTitle(t, "新会话")}
              text={sessionTitle(t, "新会话")}
            />
            <span className={`${styles.sessStatus} ${styles[status] ?? ""}`}>
              {STATUS_LABEL[status] ?? t.statusDsr}
            </span>
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
