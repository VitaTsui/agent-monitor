import React, { useCallback, useEffect, useState } from "react";

// Modal.confirm 这类命令式弹窗 hsu-ui 未提供，按约定用 antd 兜底（组件式仍用 hsu-ui 的 Modal）
import { message, Modal as AntdModal, QRCode, Spin } from "antd";
import { DingtalkOutlined, FolderOutlined, RightOutlined } from "@ant-design/icons";

import { Button, Copy, Input, Modal } from "@hsu-react/ui";

import {
  DingtalkBoundId,
  IntegrationsInfo,
  getIntegrations,
  setDingtalkApp,
  getTaskDirs,
  setDingtalkRecvDir,
  getDingtalkQr,
  getDingtalkIds,
  unbindDingtalkId,
  claimDingtalkBind,
} from "@/services/apis/portal";
import styles from "./index.module.scss";

/**
 * 机器人管理（用户端）：两条接入方式并存，各取所需。
 *
 * 1. **自己的机器人** —— 填 AppKey/AppSecret。谁配的机器人，它收到的消息就归谁、
 *    推送也只发给他，不用再绑钉钉号。
 * 2. **公共机器人** —— 用管理员配的那一个，绑定自己的钉钉号来认人：拿钉钉扫下面
 *    那个二维码，授权后即绑好。一个账号可以绑多个钉钉号（手机/电脑各一个）。
 *
 * 两者同时具备时以自己的机器人优先。另外可配「文件接收目录」（机器人收到的文件
 * 落在项目里的哪儿）。
 */
const IntegrationsPanel: React.FC = () => {
  // 自己的钉钉机器人
  const [appKey, setAppKey] = useState("");
  const [appSecret, setAppSecret] = useState("");
  const [hasSecret, setHasSecret] = useState(false);
  const [linked, setLinked] = useState(false);
  const [saving, setSaving] = useState(false);

  // 公共机器人 + 钉钉号绑定
  const [globalAvailable, setGlobalAvailable] = useState(false);
  const [boundIds, setBoundIds] = useState<DingtalkBoundId[]>([]);
  const [qr, setQr] = useState<{ url: string; command: string } | null>(null);
  const [qrLoading, setQrLoading] = useState(false);
  const [qrErr, setQrErr] = useState("");

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
        setGlobalAvailable(!!d.globalBot?.available);
        setBoundIds(d.globalBot?.boundIds ?? []);
      })
      .catch(() => void 0);
  }, []);
  useEffect(() => load(), [load]);

  // 二维码按需取：进面板就取会白白占掉一个绑定码，展开绑定区时才要
  const loadQr = useCallback(() => {
    setQrLoading(true);
    setQrErr("");
    getDingtalkQr()
      .then((res) => {
        if (res.code !== 0 || !res.data) {
          setQrErr(res.msg ?? "取二维码失败");
          return;
        }
        setQr({ url: res.data.url, command: res.data.command });
      })
      .catch(() => setQrErr("取二维码失败，请检查网络"))
      .finally(() => setQrLoading(false));
  }, []);
  useEffect(() => {
    // 没配自己的机器人、且管理员开了公共机器人时，绑定才有意义
    if (globalAvailable && !hasSecret && !qr && !qrLoading) loadQr();
  }, [globalAvailable, hasSecret, qr, qrLoading, loadQr]);

  // 扫码是在**手机上**完成的，这一端只能靠轮询知道绑上了没有。
  // 只在二维码挂着、且还没绑过时轮询，绑上即停。
  useEffect(() => {
    if (!qr || boundIds.length > 0) return;
    const timer = window.setInterval(() => {
      getDingtalkIds()
        .then((res) => {
          const list = res.data?.list ?? [];
          if (list.length > 0) {
            setBoundIds(list);
            message.success("钉钉号绑定成功");
          }
        })
        .catch(() => void 0);
    }, 3000);
    return () => window.clearInterval(timer);
  }, [qr, boundIds.length]);

  // 机器人回发的登录链接（?dtbind=<token>）：登录进来后自动认领，省得再去扫码
  useEffect(() => {
    const url = new URL(window.location.href);
    const token = url.searchParams.get("dtbind");
    if (!token) return;
    // 无论成败都把参数摘掉：刷新页面不该重复认领（token 本身也是一次性的）
    url.searchParams.delete("dtbind");
    window.history.replaceState(null, "", url.toString());
    claimDingtalkBind(token)
      .then((res) => {
        if (res.code !== 0) return message.error(res.msg ?? "绑定失败");
        message.success(`已绑定钉钉号${res.data?.nick ? ` · ${res.data.nick}` : ""}`);
        getDingtalkIds().then((r) => setBoundIds(r.data?.list ?? []));
      })
      .catch(() => message.error("绑定失败，请检查网络"));
  }, []);

  const doUnbind = (staffId: string, nick: string) => {
    AntdModal.confirm({
      title: "解绑钉钉号",
      content: `解绑后「${nick || staffId}」将不再收到推送，也不能再通过钉钉控制会话。`,
      okText: "解绑",
      cancelText: "取消",
      onOk: () =>
        unbindDingtalkId(staffId)
          .then((res) => {
            if (res.code !== 0) return message.error(res.msg ?? "解绑失败");
            message.success("已解绑");
            setBoundIds((l) => l.filter((x) => x.staffId !== staffId));
          })
          .catch(() => message.error("解绑失败，请检查网络")),
    });
  };

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

      {/* 公共机器人：不想自己建应用就绑个钉钉号，扫码即可 */}
      {globalAvailable ? (
        <div className={styles.boundCard}>
          <div className={styles.boundTitle}>
            <DingtalkOutlined className={styles.boundTitleIcon} />
            绑定钉钉号
            <span className={`${styles.botState} ${boundIds.length ? styles.botOk : ""}`}>
              {boundIds.length ? `已绑 ${boundIds.length}` : "未绑定"}
            </span>
          </div>

          {hasSecret ? (
            <div className={styles.botHint}>
              你已配了自己的机器人，推送走它就够了，无需再绑钉钉号。
            </div>
          ) : (
            <div className={styles.qrRow}>
              <div className={styles.qrBox}>
                {qrLoading ? (
                  <Spin />
                ) : qr ? (
                  <QRCode value={qr.url} size={148} bordered={false} />
                ) : (
                  <div className={styles.qrErr}>
                    {qrErr || "二维码未就绪"}
                    <Button size="small" onClick={loadQr}>
                      重试
                    </Button>
                  </div>
                )}
              </div>
              <div className={styles.qrSide}>
                <div className={styles.qrTitle}>用钉钉扫一扫</div>
                <div className={styles.qrSub}>
                  扫码授权后，这个钉钉号就绑到当前账号：会话提醒私聊推给你，
                  也能直接在钉钉里控制会话。手机、电脑可各绑一个。
                </div>
                {qr ? (
                  <div className={styles.qrAlt}>
                    扫不了？在钉钉里把这句话发给机器人也一样：
                    <code className={styles.qrCmd}>{qr.command}</code>
                    <Copy id="dt-bind-cmd" text={qr.command} />
                  </div>
                ) : null}
              </div>
            </div>
          )}

          {boundIds.length ? (
            <div className={styles.idList}>
              {boundIds.map((b) => (
                <div key={b.staffId} className={styles.idRow}>
                  <DingtalkOutlined className={styles.idIcon} />
                  <div className={styles.idName}>
                    <div className={styles.idNick}>{b.nick || "（未取到昵称）"}</div>
                    <div className={styles.idStaff}>{b.staffId}</div>
                  </div>
                  <Button size="small" onClick={() => doUnbind(b.staffId, b.nick)}>
                    解绑
                  </Button>
                </div>
              ))}
            </div>
          ) : null}
        </div>
      ) : null}

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
