import React, { useCallback, useEffect, useState } from "react";

import { Segmented, Tag, Tooltip } from "antd";
import { message } from "@hsu-react/ui";
import { CheckOutlined, CopyOutlined } from "@ant-design/icons";

import { Button, Input, Modal } from "@hsu-react/ui";

import {
  PortalDevice,
  ShareInfo,
  createShare,
  getShareGuests,
  getShareInfo,
  kickShareGuest,
  revokeShare,
} from "@/services/apis/portal";
import styles from "./index.module.scss";

interface ShareModalProps {
  device: PortalDevice | null;
  onClose: () => void;
}

/**
 * 协助共享（主人侧）：为自己的设备生成连接码 + 密码，供其他用户接入
 * （类似远程控制软件）。临时密码 30 分钟过期；固定密码长期有效。
 */
const CopyBtn: React.FC<{ text: string }> = ({ text }) => {
  const [done, setDone] = useState(false);
  const copy = () => {
    navigator.clipboard?.writeText(text).then(
      () => {
        setDone(true);
        message.success("已复制");
        window.setTimeout(() => setDone(false), 1500);
      },
      () => message.error("复制失败"),
    );
  };
  return (
    <Tooltip title="复制">
      <span className={styles.copyBtn} role="button" onClick={copy}>
        {done ? <CheckOutlined /> : <CopyOutlined />}
      </span>
    </Tooltip>
  );
};

const ShareModal: React.FC<ShareModalProps> = ({ device, onClose }) => {
  const [info, setInfo] = useState<ShareInfo | null>(null);
  const [guests, setGuests] = useState<string[]>([]);
  const [mode, setMode] = useState<"temp" | "fixed">("temp");
  const [fixedPwd, setFixedPwd] = useState("");
  const [password, setPassword] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const load = useCallback((id: string) => {
    getShareInfo(id)
      .then((res) => {
        if (res.code === 0) setInfo(res.data ?? null);
      })
      .catch(() => void 0);
    getShareGuests(id)
      .then((res) => {
        if (res.code === 0) setGuests(res.data?.list ?? []);
      })
      .catch(() => void 0);
  }, []);

  useEffect(() => {
    if (device) {
      setPassword(null);
      setFixedPwd("");
      setMode("temp");
      load(device.id);
    }
  }, [device, load]);

  const generate = () => {
    if (!device) return;
    if (mode === "fixed" && fixedPwd.trim().length < 4) {
      message.warning("固定密码至少 4 位");
      return;
    }
    setLoading(true);
    createShare(device.id, mode === "temp", mode === "fixed" ? fixedPwd.trim() : undefined)
      .then((res) => {
        if (res.code === 0 && res.data) {
          setInfo({
            code: res.data.code,
            temporary: res.data.temporary,
            expiresAt: res.data.expiresAt,
          });
          setPassword(res.data.password);
          load(device.id);
          message.success("已生成协助码");
        } else {
          message.error(res.msg ?? "生成失败");
        }
      })
      .catch(() => message.error("生成失败，请检查网络"))
      .finally(() => setLoading(false));
  };

  const stop = () => {
    if (!device) return;
    revokeShare(device.id).then((res) => {
      if (res.code === 0) {
        setInfo(null);
        setPassword(null);
        setGuests([]);
        message.success("已停止共享");
      }
    });
  };

  const kick = (user: string) => {
    if (!device) return;
    kickShareGuest(device.id, user).then((res) => {
      if (res.code === 0) load(device.id);
    });
  };

  return (
    <Modal
      title={`协助共享 · ${device?.hostname ?? ""}`}
      open={!!device}
      onCancel={onClose}
      footer={null}
      width={440}
      centered
    >
      <div className={styles.ShareModal}>
        <div className={styles.hint}>
          把连接码和密码告诉对方，对方在「接入他人设备」里输入即可查看、控制这台电脑的终端会话。随时可停止共享。
        </div>

        {info ? (
          <div className={styles.codeCard}>
            <div className={styles.codeRow}>
              <span className={styles.codeLabel}>连接码</span>
              <span className={styles.codeValue}>{info.code}</span>
              <CopyBtn text={info.code} />
            </div>
            {password ? (
              <div className={styles.codeRow}>
                <span className={styles.codeLabel}>密码</span>
                <span className={styles.codeValue}>{password}</span>
                <CopyBtn text={password} />
              </div>
            ) : (
              <div className={styles.codeRow}>
                <span className={styles.codeLabel}>密码</span>
                <span className={styles.codeMuted}>
                  {info.temporary ? "临时密码仅在生成时显示，如需请重新生成" : "固定密码已设置"}
                </span>
              </div>
            )}
            <div className={styles.codeMeta}>
              {info.temporary ? (
                <Tag color="orange">临时密码 · 30 分钟内有效</Tag>
              ) : (
                <Tag color="blue">固定密码 · 长期有效</Tag>
              )}
            </div>
            <div className={styles.actions}>
              <Button
                size="small"
                className={styles.regenBtn}
                onClick={generate}
                loading={loading}
              >
                重新生成
              </Button>
              <Button size="small" className={styles.stopBtn} onClick={stop}>
                停止共享
              </Button>
            </div>
          </div>
        ) : (
          <div className={styles.genCard}>
            <Segmented
              block
              value={mode}
              onChange={(v) => setMode(v as "temp" | "fixed")}
              options={[
                { label: "临时密码", value: "temp" },
                { label: "固定密码", value: "fixed" },
              ]}
            />
            {mode === "fixed" ? (
              <Input
                placeholder="设置固定密码（至少 4 位）"
                value={fixedPwd}
                onChange={(v) => setFixedPwd(v)}
                style={{ marginTop: 12 }}
              />
            ) : (
              <div className={styles.tempHint}>系统将生成一个 8 位数字临时密码，30 分钟内有效。</div>
            )}
            <Button
              type="primary"
              block
              loading={loading}
              onClick={generate}
              style={{ marginTop: 12 }}
            >
              生成协助码
            </Button>
          </div>
        )}

        {guests.length > 0 && (
          <div className={styles.guests}>
            <div className={styles.guestsTitle}>当前接入（{guests.length}）</div>
            {guests.map((g) => (
              <div key={g} className={styles.guestRow}>
                <span>{g}</span>
                <Button size="small" className={styles.kickBtn} type="text" onClick={() => kick(g)}>
                  移除
                </Button>
              </div>
            ))}
          </div>
        )}
      </div>
    </Modal>
  );
};

export default ShareModal;
