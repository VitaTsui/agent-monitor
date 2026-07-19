import React, { useEffect, useState } from "react";

import { message } from "antd";
import { observer } from "mobx-react-lite";

import { Button, Input, Modal, Select } from "@hsu-react/ui";

import { uploadPortalFile } from "@/services/apis/portal";
import PortalStore from "../PortalStore";
import styles from "./shareReceive.module.scss";

/** Capacitor 桥（远程壳：走全局对象，不打包插件 JS） */
interface CapBridge {
  isNativePlatform?: () => boolean;
  Plugins?: {
    App?: {
      addListener?: (
        event: string,
        cb: (data: { url?: string }) => void,
      ) => { remove?: () => void };
    };
    Filesystem?: {
      readFile?: (opts: { path: string }) => Promise<{ data: string }>;
    };
  };
}

interface SharedFile {
  path: string;
  name: string;
}

function b64ToFile(b64: string, name: string): File {
  const bin = atob(b64);
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) {
    bytes[i] = bin.charCodeAt(i);
  }
  return new File([bytes], name);
}

/**
 * 接收系统分享进来的文件（移动端壳）：
 * - Android：分享菜单 → MainActivity 拷到缓存 → window "amSharedFile" 事件；
 * - iOS：分享菜单「拷贝到 终端任务监控」→ appUrlOpen 带 file:// URL。
 * 收到后弹会话选择器，经现有上传通道传到目标设备目录。
 */
export const ShareReceiveModal: React.FC = observer(() => {
  const [shared, setShared] = useState<SharedFile | null>(null);
  const [taskId, setTaskId] = useState("");
  const [dir, setDir] = useState("");
  const [sending, setSending] = useState(false);

  const tasks = PortalStore.tasks;

  useEffect(() => {
    const cap = (window as unknown as { Capacitor?: CapBridge }).Capacitor;
    if (!cap?.isNativePlatform?.()) {
      return;
    }
    // Android 分享（自定义事件，detail 由壳注入）
    const onShared = (e: Event) => {
      const d = (e as CustomEvent).detail as SharedFile | undefined;
      if (d?.path) {
        setShared({ path: d.path, name: d.name || "shared.bin" });
      }
    };
    window.addEventListener("amSharedFile", onShared);
    // iOS「拷贝到 App」（文档打开 → appUrlOpen file://）
    const sub = cap.Plugins?.App?.addListener?.("appUrlOpen", ({ url }) => {
      if (url?.startsWith("file://")) {
        const path = decodeURIComponent(url.replace("file://", ""));
        const name = path.split("/").pop() || "shared.bin";
        setShared({ path, name });
      }
    });
    return () => {
      window.removeEventListener("amSharedFile", onShared);
      sub?.remove?.();
    };
  }, []);

  // 弹出时默认选中当前会话，目录跟随所选会话的工作目录
  useEffect(() => {
    if (!shared) {
      return;
    }
    const first = PortalStore.openIds[0] ?? tasks[0]?.id ?? "";
    setTaskId(first);
  }, [shared]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    const t = tasks.find((x) => x.id === taskId);
    setDir(t?.process?.cwd ?? "");
  }, [taskId, tasks]);

  const doUpload = () => {
    const cap = (window as unknown as { Capacitor?: CapBridge }).Capacitor;
    const read = cap?.Plugins?.Filesystem?.readFile;
    const t = tasks.find((x) => x.id === taskId);
    if (!shared || !read || !t?.machineId || !dir.trim()) {
      message.warning("请选择目标会话与目录");
      return;
    }
    const machineId = t.machineId;
    setSending(true);
    read({ path: shared.path })
      .then(({ data }) => {
        const file = b64ToFile(data, shared.name);
        return uploadPortalFile(machineId, dir.trim(), file);
      })
      .then((res) => {
        if (res.code === 0) {
          message.success(`已传到 ${t.hostname ?? "设备"}：${shared.name}`);
          setShared(null);
        } else {
          message.error(res.msg ?? "上传失败");
        }
      })
      .catch(() => message.error("读取或上传失败"))
      .finally(() => setSending(false));
  };

  return (
    <Modal
      title="收到分享的文件"
      open={!!shared}
      onCancel={() => setShared(null)}
      footer={null}
      width={460}
      centered
    >
      <div className={styles.shareForm}>
        <div className={styles.fileName}>
          文件：<b>{shared?.name}</b>
        </div>
        <div className={styles.label}>传到哪个会话所在的设备</div>
        <Select
          value={taskId || undefined}
          onChange={(v) => setTaskId(String(v ?? ""))}
          options={tasks.map((t) => ({
            label: `${t.hostname ?? t.machineId} · ${
              t.title || t.providerDsr || "会话"
            }`,
            value: t.id ?? "",
          }))}
          placeholder="选择会话"
          style={{ width: "100%" }}
        />
        <div className={styles.label}>目标目录（默认该会话所在目录）</div>
        <Input value={dir} onChange={(v) => setDir(v)} placeholder="目标目录" />
        <div className={styles.actions}>
          <Button type="primary" loading={sending} onClick={doUpload}>
            上传
          </Button>
          <Button onClick={() => setShared(null)}>取消</Button>
        </div>
      </div>
    </Modal>
  );
});
