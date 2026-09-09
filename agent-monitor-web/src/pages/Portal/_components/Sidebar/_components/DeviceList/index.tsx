import React from "react";

import { LaptopOutlined } from "@ant-design/icons";
import { observer } from "mobx-react-lite";

import PortalStore from "../../../../PortalStore";
import ScrollText from "../../../ScrollText";
import styles from "./index.module.scss";

interface DeviceListProps {
  /** 客户端窗口内的本机 machineId（浏览器里为 null，不标「本机」） */
  localId: string | null;
}

/** 侧栏的设备选择区。 */
const DeviceList: React.FC<DeviceListProps> = observer(({ localId }) => {
  const { deviceList, selectedMachineId, selectMachine } = PortalStore;

  if (deviceList.length === 0) {
    return (
      <div className={styles.deviceBar}>
        <div className={styles.noDevice}>暂无设备</div>
      </div>
    );
  }

  return (
    <div className={styles.deviceBar}>
      {deviceList.map((d) => (
        <div
          key={d.machineId}
          className={`${styles.deviceTab} ${
            d.machineId === selectedMachineId ? styles.active : ""
          }`}
          role="button"
          tabIndex={0}
          aria-pressed={d.machineId === selectedMachineId}
          onClick={() => selectMachine(d.machineId)}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              selectMachine(d.machineId);
            }
          }}
          title={`${d.hostname} · ${d.platformDsr}${
            d.running > 0 ? ` · ${d.running} 执行中` : ""
          }`}
        >
          {/* 统一用电脑图标(像 iOS 设备列表)，不再在名称前加平台 emoji */}
          <LaptopOutlined />
          <ScrollText
            className={styles.deviceTabName}
            active={d.machineId === selectedMachineId}
            plain={d.hostname}
            text={
              <>
                {d.hostname}
                {d.machineId === localId ? (
                  <span className={styles.localTag}>本机</span>
                ) : null}
              </>
            }
          />
          <span className={styles.deviceTabStat}>{d.count} 会话</span>
          {d.running > 0 ? <span className={styles.deviceTabDot} /> : null}
        </div>
      ))}
    </div>
  );
});

export default DeviceList;
