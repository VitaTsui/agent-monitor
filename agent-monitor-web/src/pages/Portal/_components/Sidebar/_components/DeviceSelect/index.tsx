import React from "react";

import { Dropdown, Tooltip } from "antd";
import { CheckOutlined, DownOutlined, LaptopOutlined } from "@ant-design/icons";
import { observer } from "mobx-react-lite";

import PortalStore from "../../../../PortalStore";
import styles from "./index.module.scss";

/**
 * 侧栏顶部的**设备选择器**：一行「在看哪台机器」，点开换一台。
 *
 * 设备这一维从会话分组里**提出来**了。上一版是「设备 × 客户端」二合一，组标题写成
 * `MacBook Pro · Claude Code`：机器一多，同一个客户端名在一列里重复出现好几遍，
 * 而「我现在在看哪台机器」没有任何一处说得清 —— 只能靠一行行读组标题去拼。
 * 现在设备在顶部选、下面只按客户端（`Claude Code` / `Codex` / `ChatGPT 桌面版`）分组，
 * 组标题里不再带主机名。
 *
 * **只有一台设备时不做成按钮**：没有第二个可选项，给它一颗能点开的下拉纯属骗人
 * ——点开只会看到刚才那一台。那时它退成一行说明「你在看这台机器」：无悬停底、
 * 无箭头、不可聚焦，但**仍然显示**（否则单机用户永远不知道列表是按设备分的，
 * 接了第二台机器时也解释不了这一栏为什么突然冒出来）。
 *
 * 这一块接的是被 `e094716` 删掉的那个 `DeviceList`，它那几样能力一件没丢：
 * 在线状态（那边是绿点，这里是行内状态点 ＋ 菜单里的「离线」二字）、
 * 「本机」徽标、主机名、会话数、执行中条数。多出来的是：菜单形态（列表不再
 * 长期占掉侧栏顶部好几行）、选中项持久化。
 */
const DeviceSelect: React.FC = observer(() => {
  const { deviceList, selectedMachineId, selectMachine } = PortalStore;

  if (deviceList.length === 0) {
    /* 一台设备都没有。**不画成可点的行** —— 没有任何东西可选。
       具体怎么接客户端由下面的列表空态负责说（那儿有地方写完整的一句话）。 */
    return <div className={styles.empty}>暂无设备</div>;
  }

  const current =
    deviceList.find((d) => d.machineId === selectedMachineId) ?? deviceList[0];
  const single = deviceList.length === 1;

  /** 行内那一枚：主机名 ＋「本机」＋ 在线点。选择器与菜单项共用，两处必须长一样 */
  const face = (
    <>
      <LaptopOutlined className={styles.icon} />
      <span className={styles.name}>{current.hostname}</span>
      {current.isLocal ? <span className={styles.localTag}>本机</span> : null}
      {/* 离线设备照样能选：它的历史会话仍然看得到，只是正文取不回来
          （见 ChatPane 的离线提示）。所以这里说「离线」，不是禁用它 */}
      {current.online ? null : <span className={styles.offTag}>离线</span>}
      {current.running > 0 ? <span className={styles.dot} /> : null}
    </>
  );

  /**
   * 会话数**必须带上口径**：它是客户端回溯窗口（`AM_HISTORY_DAYS`，默认 30 天）
   * 内的条数，不是「列表里有这么多条」。
   *
   * 不带限定词的那句「64 会话」是在说假话：CLI 那一列现在只列**当前打开的**终端
   * （见 PortalStore.clientSections），点开往往只有两三条；桌面那一列也要翻页
   * 才铺得出来。用户原话：「会话数量不对」。
   *
   * 为什么不改成「实际会列出的条数」：那个数对**没选中的设备**根本不存在 ——
   * 桌面客户端列的是历史，而历史只有点开那一组才会去拉；这里要给列表里每一台
   * 设备都印一个数，只能用随 `/monitor/devices` 一起下发的这个。
   * 与其印一个算不准的，不如把印的这个说清楚是什么。
   */
  const countDsr = (n: number) => `近 30 天 ${n} 条`;

  const title = `${current.hostname} · ${current.platformDsr}${
    current.online ? "" : " · 离线"
  } · ${countDsr(current.count)}${
    current.running > 0 ? ` · ${current.running} 执行中` : ""
  }`;

  if (single) {
    return (
      <div className={styles.DeviceSelect}>
        <Tooltip title={title} placement="right">
          <div className={`${styles.row} ${styles.rowStatic}`}>{face}</div>
        </Tooltip>
      </div>
    );
  }

  /* 当前那一台用**行尾一枚勾**标出来，不用 antd 的 `selectable`。
     那套的选中态是整行铺 `controlItemBgActive`，而本项目的主色是墨黑 ——
     推导出来的底色是一块近黑的实心填充，行里的次级灰（会话数）与「本机」徽标
     压在上面全看不见。一枚勾在深浅两套主题下都读得出来。 */
  const items = deviceList.map((d) => ({
    key: d.machineId,
    icon: <LaptopOutlined />,
    label: (
      <span className={styles.menuRow}>
        <span className={styles.menuName}>{d.hostname}</span>
        {d.isLocal ? <span className={styles.localTag}>本机</span> : null}
        <span className={styles.menuMeta}>
          {d.online ? countDsr(d.count) : "离线"}
          {d.running > 0 ? ` · ${d.running} 执行中` : ""}
        </span>
        {d.machineId === current.machineId ? (
          <CheckOutlined className={styles.menuCheck} />
        ) : null}
      </span>
    ),
    onClick: () => selectMachine(d.machineId),
  }));

  return (
    <div className={styles.DeviceSelect}>
      <Dropdown
        rootClassName="va-menu"
        trigger={["click"]}
        placement="bottomLeft"
        menu={{ items }}
      >
        <button
          type="button"
          className={styles.row}
          aria-label={`当前设备：${current.hostname}，点击切换`}
          title={title}
        >
          {face}
          <DownOutlined className={styles.caret} />
        </button>
      </Dropdown>
    </div>
  );
});

export default DeviceSelect;
