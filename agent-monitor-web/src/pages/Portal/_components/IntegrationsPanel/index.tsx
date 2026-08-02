import React, { useCallback, useEffect, useState } from "react";

import { message } from "antd";
import { DingtalkOutlined, FolderOutlined, RightOutlined } from "@ant-design/icons";

import { Button, Input, Modal } from "@hsu-react/ui";

import {
  IntegrationsInfo,
  getIntegrations,
  setDingtalkApp,
  getTaskDirs,
  setDingtalkRecvDir,
} from "@/services/apis/portal";
import styles from "./index.module.scss";

/**
 * 机器人管理（用户端）：在这里配置**自己的**钉钉机器人。
 *
 * 一个账号一个机器人：谁配的机器人，它收到的消息就归谁、推送也只发给他 ——
 * 不需要再单独去绑定自己的钉钉 id，也没有一个机器人服务多人那套。
 * 另外可配「文件接收目录」（机器人收到的文件落在项目里的哪儿）。
 */
const IntegrationsPanel: React.FC = () => {
  // 自己的钉钉机器人
  const [appKey, setAppKey] = useState("");
  const [appSecret, setAppSecret] = useState("");
  const [hasSecret, setHasSecret] = useState(false);
  const [linked, setLinked] = useState(false);
  const [saving, setSaving] = useState(false);

  // 机器人文件接收目录（通用）：按设备分组的项目
  type RecvProj = { cwd: string; name: string; dir: string; taskId?: string | null };
  const [recvDevices, setRecvDevices] = useState<
    { machineId: string; hostname: string; projects: RecvProj[] }[]
  >([]);
  const [recvModalOpen, setRecvModalOpen] = useState(false);
  // 每个项目目录输入框的当前值（cwd → 目录）；可手输或由「浏览」回填
  const [recvEdits, setRecvEdits] = useState<Record<string, string>>({});
  const recvConfiguredCount = recvDevices.reduce(
    (n, d) => n + d.projects.filter((p) => p.dir).length,
    0
  );
  // 目录选择器：为哪个项目开着 + 浏览态
  const [picker, setPicker] = useState<{
    cwd: string;
    name: string;
    taskId: string;
  } | null>(null);
  const [pickRel, setPickRel] = useState("");
  const [pickDirs, setPickDirs] = useState<string[]>([]);
  const [pickLoading, setPickLoading] = useState(false);
  const pickSeq = React.useRef(0);

  const loadPickDirs = useCallback(
    (taskId: string, rel: string, attempt = 0) => {
      const seq = ++pickSeq.current;
      setPickLoading(true);
      getTaskDirs(taskId, rel)
        .then((res) => {
          if (seq !== pickSeq.current) return;
          if (res.code !== 0) {
            setPickLoading(false);
            message.error(res.msg ?? "读取目录失败");
            return;
          }
          if (res.data?.pending && attempt < 8) {
            window.setTimeout(() => loadPickDirs(taskId, rel, attempt + 1), 1200);
            return;
          }
          setPickDirs(res.data?.dirs ?? []);
          setPickLoading(false);
        })
        .catch(() => seq === pickSeq.current && setPickLoading(false));
    },
    []
  );

  const openPicker = (p: RecvProj) => {
    if (!p.taskId) {
      message.info("该项目当前无活跃会话，无法浏览目录；可等它有会话后再选");
      return;
    }
    // 打开时定位到「当前选中目录的父级」，方便看到它和同级目录。
    // 绝对路径 / 无法在项目树里相对浏览 → 从项目根开始。
    const cur = (recvEdits[p.cwd] ?? p.dir).trim().replace(/^\.\//, "");
    const isAbs = cur.startsWith("/") || /^[a-zA-Z]:[\\/]/.test(cur);
    const startRel =
      cur && !isAbs ? cur.split("/").filter(Boolean).slice(0, -1).join("/") : "";
    setPicker({ cwd: p.cwd, name: p.name, taskId: p.taskId });
    setPickRel(startRel);
    setPickDirs([]);
    loadPickDirs(p.taskId, startRel);
  };

  const commitRecvDir = (cwd: string, dir: string) => {
    setDingtalkRecvDir(cwd, dir)
      .then((res) => {
        if (res.code !== 0) return message.error(res.msg ?? "保存失败");
        message.success(dir ? "已保存" : "已清除，回落默认 tmp");
        setRecvDevices((devs) =>
          devs.map((d) => ({
            ...d,
            projects: d.projects.map((x) => (x.cwd === cwd ? { ...x, dir } : x)),
          }))
        );
        setPicker(null);
      })
      .catch(() => message.error("保存失败，请检查网络"));
  };

  const load = useCallback(() => {
    getIntegrations()
      .then((res) => {
        if (res.code !== 0 || !res.data) return;
        const d: IntegrationsInfo = res.data;
        const devs = (d.recvDirDevices ?? []).map((dev) => ({
          machineId: dev.machineId,
          hostname: dev.hostname,
          projects: dev.projects.map((p) => ({
            cwd: p.cwd,
            name: p.name,
            dir: p.dir ?? "",
            taskId: p.taskId,
          })),
        }));
        setRecvDevices(devs);
        // 输入框初值 = 各项目已配目录
        const edits: Record<string, string> = {};
        devs.forEach((dev) => dev.projects.forEach((p) => (edits[p.cwd] = p.dir)));
        setRecvEdits(edits);
        // 自己的机器人（密钥不回显，只知道配没配）
        setAppKey(d.dingtalk?.appKey ?? "");
        setHasSecret(!!d.dingtalk?.hasSecret);
        setLinked(!!d.dingtalk?.linked);
      })
      .catch(() => void 0);
  }, []);
  useEffect(() => load(), [load]);

  const saveApp = () => {
    const key = appKey.trim();
    if (key && !appSecret.trim() && !hasSecret) {
      return message.error("请填写 AppSecret");
    }
    setSaving(true);
    setDingtalkApp({ appKey: key, appSecret: appSecret.trim() })
      .then((res) => {
        if (res.code !== 0) return message.error(res.msg ?? "保存失败");
        message.success(res.data?.result ?? "已保存");
        setAppSecret("");
        setHasSecret(!!key);
        if (!key) setLinked(false);
      })
      .catch(() => message.error("保存失败，请检查网络"))
      .finally(() => setSaving(false));
  };

  return (
    <div className={styles.IntegrationsPanel}>
      {/* 文件接收目录（置顶） */}
      <div className={styles.groupTitle}>文件接收目录</div>
      <div
        className={styles.card}
        role="button"
        tabIndex={0}
        onClick={() => setRecvModalOpen(true)}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            setRecvModalOpen(true);
          }
        }}
      >
        <span className={`${styles.icon} ${styles.folder}`}>
          <FolderOutlined />
        </span>
        <div className={styles.headText}>
          <div className={styles.headTitle}>机器人文件接收目录</div>
          <div className={styles.headSub}>
            发给机器人的文件落到哪 · 按设备/项目配置 · 默认 tmp
          </div>
        </div>
        <span className={styles.status}>
          {recvConfiguredCount ? `已配 ${recvConfiguredCount}` : "默认"}
        </span>
        <RightOutlined className={styles.arrow} />
      </div>

      {/* 自己的钉钉机器人：一个账号一个，配好即归自己 */}
      <div className={styles.groupTitle}>钉钉</div>
      <div className={styles.boundCard}>
        <div className={styles.boundTitle}>
          <DingtalkOutlined className={styles.boundTitleIcon} />
          钉钉机器人
          {hasSecret ? (
            <span
              className={`${styles.botState} ${linked ? styles.botOk : ""}`}
              // 配好了但还没人跟它说过话时，hub 不知道该把推送发给谁
              title={linked ? "已连通，推送会私聊发给你" : "还没收到过你的消息"}
            >
              {linked ? "已连通" : "待发首条消息"}
            </span>
          ) : null}
        </div>
        <div className={styles.botForm}>
          <Input
            placeholder="AppKey（钉钉应用的 ClientID）"
            value={appKey}
            onChange={(v: string) => setAppKey(v)}
          />
          <Input
            type="password"
            placeholder={hasSecret ? "AppSecret（已保存，留空则不改）" : "AppSecret"}
            value={appSecret}
            onChange={(v: string) => setAppSecret(v)}
          />
          <div className={styles.botActions}>
            <Button type="primary" loading={saving} onClick={saveApp}>
              保存
            </Button>
            {hasSecret ? (
              <Button
                loading={saving}
                onClick={() => {
                  setAppKey("");
                  setAppSecret("");
                  setDingtalkApp({ appKey: "" })
                    .then(() => {
                      message.success("已解绑");
                      setHasSecret(false);
                      setLinked(false);
                    })
                    .catch(() => message.error("解绑失败"));
                }}
              >
                解绑
              </Button>
            ) : null}
          </div>
          <div className={styles.botHint}>
            在钉钉开放平台建一个「企业内部应用 · 机器人」，开启 Stream
            模式，把 ClientID / ClientSecret 填到这里。保存后去钉钉给机器人发句话，
            它就知道该把消息推给谁了。
          </div>
        </div>
      </div>

      {/* 接收目录配置弹窗：设备 → 项目 层级列全 */}
      <Modal
        title="机器人文件接收目录"
        open={recvModalOpen}
        onCancel={() => setRecvModalOpen(false)}
        footer={null}
        width={560}
        centered
      >
        <div className={styles.recvModalHint}>
          发给机器人的文件，随下一条任务落到对应会话项目的这个目录。
          留空 = 默认 <code>项目/tmp</code>。点「浏览」在项目目录树里选。
        </div>
        {recvDevices.length === 0 ? (
          <div className={styles.recvEmpty}>暂无项目（有活跃会话后自动出现）</div>
        ) : (
          recvDevices.map((dev) => (
            <div key={dev.machineId || dev.hostname} className={styles.recvDev}>
              <div className={styles.recvDevName}>💻 {dev.hostname}</div>
              {dev.projects.map((p) => (
                <div key={p.cwd} className={styles.recvProjRow}>
                  <div className={styles.recvProjName} title={p.cwd}>
                    <div className={styles.recvProjTitle}>{p.name}</div>
                    <div className={styles.recvProjPath}>{p.cwd}</div>
                  </div>
                  <div className={styles.recvProjEdit}>
                    <Input
                      className={styles.recvProjInput}
                      placeholder="tmp（默认）· 可直接输入或点浏览"
                      value={recvEdits[p.cwd] ?? p.dir}
                      onChange={(v) => setRecvEdits((m) => ({ ...m, [p.cwd]: v }))}
                    />
                    <Button size="small" onClick={() => openPicker(p)}>
                      浏览
                    </Button>
                    <Button
                      size="small"
                      type="primary"
                      onClick={() => commitRecvDir(p.cwd, (recvEdits[p.cwd] ?? "").trim())}
                    >
                      保存
                    </Button>
                  </div>
                </div>
              ))}
            </div>
          ))
        )}
      </Modal>

      {/* 目录浏览选择弹窗（复用会话目录树浏览） */}
      <Modal
        title={picker ? `选择接收目录 · ${picker.name}` : "选择接收目录"}
        open={!!picker}
        onCancel={() => setPicker(null)}
        onOk={() => {
          // 回填到该项目输入框（不立即保存，可再改，再点保存）
          if (picker) setRecvEdits((m) => ({ ...m, [picker.cwd]: pickRel }));
          setPicker(null);
        }}
        okText={`用此目录（${pickRel || "项目根"}）`}
        cancelText="取消"
        width={480}
        centered
      >
        <div className={styles.pickPath}>
          <span
            className={styles.pickCrumb}
            role="button"
            tabIndex={0}
            onClick={() => {
              setPickRel("");
              picker && loadPickDirs(picker.taskId, "");
            }}
          >
            项目根
          </span>
          {pickRel
            ? pickRel.split("/").map((seg, i, arr) => {
                const rel = arr.slice(0, i + 1).join("/");
                return (
                  <span key={rel}>
                    <span className={styles.pickSep}>/</span>
                    <span
                      className={styles.pickCrumb}
                      role="button"
                      tabIndex={0}
                      onClick={() => {
                        setPickRel(rel);
                        picker && loadPickDirs(picker.taskId, rel);
                      }}
                    >
                      {seg}
                    </span>
                  </span>
                );
              })
            : null}
        </div>
        <div className={styles.pickList}>
          {pickLoading ? (
            <div className={styles.pickLoading}>读取中…</div>
          ) : pickDirs.length === 0 ? (
            <div className={styles.pickEmpty}>此目录下没有子目录</div>
          ) : (
            pickDirs.map((name) => (
              <div
                key={name}
                className={styles.pickItem}
                role="button"
                tabIndex={0}
                onClick={() => {
                  const next = pickRel ? `${pickRel}/${name}` : name;
                  setPickRel(next);
                  picker && loadPickDirs(picker.taskId, next);
                }}
              >
                <FolderOutlined className={styles.pickIcon} />
                <span className={styles.pickItemName}>{name}</span>
                <RightOutlined className={styles.pickArrow} />
              </div>
            ))
          )}
        </div>
      </Modal>
    </div>
  );
};

export default IntegrationsPanel;
