import React, { useEffect, useRef, useState } from "react";

import { Modal } from "@hsu-react/ui";
import {
  BgColorsOutlined,
  CloseOutlined,
  CloudSyncOutlined,
  ControlOutlined,
  EnterOutlined,
  InfoCircleOutlined,
  LaptopOutlined,
  MessageOutlined,
  RobotOutlined,
  SafetyOutlined,
  SearchOutlined,
  SettingOutlined,
  UnorderedListOutlined,
} from "@ant-design/icons";
import { observer } from "mobx-react-lite";
import { useNavigate } from "react-router-dom";

// 外观三态的真源，与设置里的「外观」分栏同一个单例（见 SettingsModal/panes/AppearancePane）。
// 走深路径而不是 barrel：那会把只有后管才用的 Header / Menu 一起拉进前台的包。
import ThemeStore from "@hsu-react/ui/es/layout/Theme/ThemeStore";

import { inNativeShell } from "@/utils/clientAuth";
import PortalStore from "../../PortalStore";
import { usePortalUser } from "../../_context/portalUser";
import {
  HISTORY_LIST_PATH,
  PORTAL_BASE,
  type SettingsTab,
} from "../../_utils/portalNav";
import { sessionTitle } from "../../_utils/sessionNote";
import styles from "./index.module.scss";

interface SearchPaletteProps {
  open: boolean;
  onClose: () => void;
  /** 选中会话（与侧栏会话行同一个动作：换成单格 ＋ 顺带回会话页） */
  onSelectSession: (id: string) => void;
  /** 进设置的某个分栏（与侧栏账户菜单、设置弹窗左栏同一个动作） */
  onOpenSettings: (tab: SettingsTab) => void;
}

type Hit = {
  key: string;
  icon: React.ReactNode;
  title: string;
  /** 行尾那一小段：设备 / 状态 / 会话数 */
  meta: string;
  /** 非会话的那两类各挂一枚小标签，混排时一眼分得清 */
  tag?: string;
  /** 参与匹配但不显示的别名（命令用：中文名之外还能按拼音以外的近义词搜到） */
  alias?: string;
  run: () => void;
};

const STATUS_LABEL: Record<string, string> = {
  running: "执行中",
  idle: "等待输入",
  paused: "已暂停",
  finished: "已结束",
};

/**
 * 全局命令面板（⌘K / Ctrl+K）。
 *
 * **它只是现有入口的另一种打开方式**，不新增任何能力：面板里每一条都能在界面上
 * 找到对应的地方 —— 会话来自侧栏那一列、设备来自侧栏的设备区、命令来自
 * 「查看全部会话」那一行、账户菜单与设置弹窗的各个分栏、外观分栏那个三选一。
 *
 * 与侧栏原来那个搜索框**不是两件事，是同一件事的两个入口**，所以那个框已经撤掉、
 * 换成侧栏头部一颗搜索按钮（见 Sidebar）。理由：那个框只筛「当前选中设备下的会话」，
 * 这里搜的是**全部设备的全部会话**，还能搜到设备与设置项；两个搜索摆在一起，
 * 用户会以为它们搜的是同一份东西，实际结果却对不上。
 *
 * 数据全在本地（会话与设备列表壳里已经拉过），所以不发请求、边打字边出结果。
 */
const SearchPalette: React.FC<SearchPaletteProps> = observer((props) => {
  const { open, onClose, onSelectSession, onOpenSettings } = props;
  const user = usePortalUser();
  const navigate = useNavigate();
  const [kw, setKw] = useState("");
  const [active, setActive] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    setKw("");
    setActive(0);
    // Modal 有进场动画，立刻 focus 会落空
    const t = setTimeout(() => inputRef.current?.focus(), 80);
    return () => clearTimeout(t);
  }, [open]);

  /**
   * **不要用 `useMemo`**：会话与设备两份清单是 MobX 的 observable，挂载之后才拉回来。
   * 用 `useMemo(..., [kw])` 缓存的是「首次渲染那一刻」的结果 —— 那时列表还是空的，
   * 于是面板永远显示「没有匹配的内容」。这点计算量每次渲染都做也无所谓。
   */
  const hits: Hit[] = (() => {
    const k = kw.trim().toLowerCase();
    const match = (...fields: (string | null | undefined)[]) =>
      !k || fields.some((s) => (s ?? "").toLowerCase().includes(k));

    /* 命令：对应界面上已有的那些入口，一个不多 */
    const commands: Hit[] = [
      {
        key: "cmd-history",
        icon: <UnorderedListOutlined />,
        title: "查看全部会话",
        meta: "跨设备的全量记录",
        alias: "history 历史 远程往来",
        run: () => navigate(HISTORY_LIST_PATH),
      },
      ...(
        [
          ["account", "账户", <SettingOutlined key="i" />, "account 个人资料 改密码"],
          ["appearance", "外观", <BgColorsOutlined key="i" />, "appearance 主题 深色 浅色"],
          ["devices", "设备管理", <LaptopOutlined key="i" />, "devices 电脑 信任 配对"],
          ["configs", "配置同步", <CloudSyncOutlined key="i" />, "configs 同步"],
          ["bots", "机器人管理", <RobotOutlined key="i" />, "bots 钉钉 机器人接入"],
          ["security", "安全防护", <SafetyOutlined key="i" />, "security 安全"],
          ["about", "关于", <InfoCircleOutlined key="i" />, "about 版本 更新"],
        ] as [SettingsTab, string, React.ReactNode, string][]
      ).map(([tab, label, icon, alias]) => ({
        key: `cmd-set-${tab}`,
        icon,
        title: `设置 · ${label}`,
        meta: "打开设置",
        alias,
        run: () => onOpenSettings(tab),
      })),
      ...(
        [
          ["light", "浅色"],
          ["dark", "深色"],
          ["system", "跟随系统"],
        ] as const
      ).map(([value, label]) => ({
        key: `cmd-theme-${value}`,
        icon: <BgColorsOutlined />,
        title: `外观 · 切换到${label}`,
        meta: ThemeStore.appearance === value ? "当前" : "",
        alias: "theme appearance 主题 明暗",
        run: () => ThemeStore.setAppearance(value),
      })),
      // 后台管理只在浏览器里有入口：客户端 / 移动端原生壳内开新标签打不开后管，
      // 与侧栏账户菜单同一条判断
      ...(user.isSuper && !inNativeShell()
        ? [
            {
              key: "cmd-admin",
              icon: <ControlOutlined />,
              title: "后台管理",
              meta: "新标签打开",
              alias: "admin 后管 用户管理",
              run: () => window.open("/admin", "_blank"),
            },
          ]
        : []),
    ].filter((c) => match(c.title, c.alias));

    /* 设备：侧栏那一区的同一份数据，选中即切左栏筛选 */
    const devices: Hit[] = PortalStore.deviceList
      .filter((d) => match(d.hostname, d.platformDsr))
      .map((d) => ({
        key: `dev-${d.machineId}`,
        icon: <LaptopOutlined />,
        title: d.hostname,
        meta: `${d.count} 会话${d.running > 0 ? ` · ${d.running} 执行中` : ""}`,
        tag: "设备",
        run: () => {
          PortalStore.selectMachine(d.machineId);
          navigate(PORTAL_BASE);
        },
      }));

    /* 会话：**全部设备**的全量，匹配字段与 PortalStore 那份筛选逐条对齐 */
    const sessions: Hit[] = PortalStore.tasks
      .filter((t) => match(t.note, t.title, t.prompt, t.projectName, t.hostname))
      .map((t) => ({
        key: `task-${t.id}`,
        icon: <MessageOutlined />,
        title: sessionTitle(t, "新会话"),
        meta: [t.hostname, STATUS_LABEL[t.status ?? ""] ?? t.statusDsr]
          .filter(Boolean)
          .join(" · "),
        run: () => onSelectSession(t.id ?? ""),
      }));

    /* 顺序：命令 → 设备 → 会话。前两类**总共也没几条**，排在后面就被几十条会话
       冲到看不见的地方，而它们恰恰是不搜就得翻好几层才点得到的东西 */
    return [...commands, ...devices, ...sessions].slice(0, 40);
  })();

  useEffect(() => {
    setActive(0);
  }, [kw]);

  const go = (hit?: Hit) => {
    if (!hit) return;
    onClose();
    hit.run();
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive((i) => Math.min(i + 1, hits.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      go(hits[active]);
    }
  };

  /** 选中项要跟着键盘滚进视野，否则按到第十条以后就看不见了 */
  useEffect(() => {
    const el = listRef.current?.querySelector(`[data-i="${active}"]`);
    el?.scrollIntoView({ block: "nearest" });
  }, [active]);

  return (
    <Modal
      open={open}
      onCancel={onClose}
      footer={null}
      title={null}
      closable={false}
      // 顶部对齐而不是垂直居中：结果条数一变，居中的面板会整体上下跳
      centered={false}
      moveable={false}
      width={672}
      style={{ top: 120 }}
      // 内边距走内联而不是样式表：组件库给 `.ant-modal-content` 写了 `0 0 15px`，
      // 同权重的类选择器谁赢取决于样式注入顺序，内联才是确定的
      styles={{ container: { padding: 0, overflow: "hidden" }, body: { padding: 0 } }}
      className={styles.SearchPalette}
      destroyOnHidden
    >
      <div className={styles.head}>
        <SearchOutlined className={styles.headIcon} />
        <input
          ref={inputRef}
          className={styles.input}
          placeholder="搜会话、设备，或输入命令"
          value={kw}
          onChange={(e) => setKw(e.target.value)}
          onKeyDown={onKey}
        />
        <button
          type="button"
          className={styles.close}
          onClick={onClose}
          aria-label="关闭"
          title="关闭（Esc）"
        >
          <CloseOutlined />
        </button>
      </div>

      <div className={styles.list} ref={listRef}>
        {hits.length === 0 ? (
          <div className={styles.blank}>没有匹配的内容</div>
        ) : (
          hits.map((h, i) => (
            <button
              type="button"
              key={h.key}
              data-i={i}
              className={i === active ? styles.rowOn : styles.row}
              /* 用 mousemove 而不是 mouseenter：面板是键盘唤起的，指针多半正停在
                 面板将要盖住的位置上 —— mouseenter 会在开面板那一瞬间就把当前项
                 从第一条挪到指针底下那条，回车于是开了个用户没看的东西。
                 mousemove 只在指针真的动了才响应。 */
              onMouseMove={() => setActive(i)}
              onClick={() => go(h)}
            >
              <span className={styles.rowIcon}>{h.icon}</span>
              <span className={styles.rowNm}>{h.title}</span>
              {h.tag && <span className={styles.rowTag}>{h.tag}</span>}
              {h.meta && <span className={styles.rowMeta}>{h.meta}</span>}
              {/* 选中那行右端提示回车可开 */}
              {i === active && (
                <span className={styles.rowEnter}>
                  <EnterOutlined />
                </span>
              )}
            </button>
          ))
        )}
      </div>
    </Modal>
  );
});

export default SearchPalette;
