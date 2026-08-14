import React, { useCallback, useEffect, useState } from "react";

import { Popconfirm, Tag } from "antd";
import { message } from "@hsu-react/ui";
import { PlusOutlined } from "@ant-design/icons";

import {
  Button,
  ColumnsType,
  Descriptions,
  Input,
  Modal,
  Panel,
  Table,
} from "@hsu-react/ui";

import {
  ChangelogEntry,
  addChangelog,
  delChangelog,
  getVersionAdminInfo,
  setVersionMinimum,
} from "@/services/apis/sysmgmt/version";
import styles from "./index.module.scss";

/**
 * 版本管理：查看各端当前版本、设置强制更新下限、维护更新日志。
 * 强制更新下限写入下载目录 manifest.json，客户端/移动端下一次心跳即生效。
 */
const Version: React.FC = () => {
  const [loading, setLoading] = useState(false);
  const [desktop, setDesktop] = useState("");
  const [android, setAndroid] = useState<string | null>(null);
  const [desktopMin, setDesktopMin] = useState("");
  const [androidMin, setAndroidMin] = useState("");
  const [savingMin, setSavingMin] = useState(false);
  const [changelog, setChangelog] = useState<ChangelogEntry[]>([]);
  // 新增日志弹窗
  const [addOpen, setAddOpen] = useState(false);
  const [addVersion, setAddVersion] = useState("");
  const [addNotes, setAddNotes] = useState("");
  const [adding, setAdding] = useState(false);

  const load = useCallback(() => {
    setLoading(true);
    getVersionAdminInfo()
      .then((res) => {
        if (res.code === 0 && res.data) {
          setDesktop(res.data.desktop);
          setAndroid(res.data.android);
          setDesktopMin(res.data.desktopMin ?? "");
          setAndroidMin(res.data.androidMin ?? "");
          setChangelog(res.data.changelog ?? []);
        } else {
          message.error(res.msg ?? "加载失败");
        }
      })
      .catch(() => message.error("加载失败，请检查网络"))
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const saveMin = () => {
    setSavingMin(true);
    setVersionMinimum({ desktopMin: desktopMin.trim(), androidMin: androidMin.trim() })
      .then((res) => {
        if (res.code === 0) {
          message.success("已保存，客户端下一次心跳即生效");
          load();
        } else {
          message.error(res.msg ?? "保存失败");
        }
      })
      .catch(() => message.error("保存失败，请检查网络"))
      .finally(() => setSavingMin(false));
  };

  const doAdd = () => {
    if (!addVersion.trim() || !addNotes.trim()) {
      message.warning("请填写版本号与更新说明");
      return;
    }
    setAdding(true);
    addChangelog({ version: addVersion.trim(), date: "", notes: addNotes.trim() })
      .then((res) => {
        if (res.code === 0) {
          message.success("已添加");
          setAddOpen(false);
          setAddVersion("");
          setAddNotes("");
          load();
        } else {
          message.error(res.msg ?? "添加失败");
        }
      })
      .catch(() => message.error("添加失败，请检查网络"))
      .finally(() => setAdding(false));
  };

  const columns: ColumnsType<ChangelogEntry> = [
    {
      title: "版本",
      dataIndex: "version",
      width: 120,
      render: (v: string) => <Tag color="blue">v{v}</Tag>,
    },
    { title: "日期", dataIndex: "date", width: 140 },
    {
      title: "更新说明",
      dataIndex: "notes",
      render: (v: string) => <span className={styles.notes}>{v}</span>,
    },
    {
      title: "操作",
      dataIndex: "op",
      width: 90,
      render: (_: unknown, row: ChangelogEntry) => (
        <Popconfirm
          title={`删除 v${row.version} 的日志？`}
          okText="删除"
          cancelText="取消"
          onConfirm={() => {
            delChangelog(row.version).then((res) => {
              if (res.code === 0) {
                message.success("已删除");
                load();
              } else {
                message.error(res.msg ?? "删除失败");
              }
            });
          }}
        >
          <Button size="small" danger type="text">
            删除
          </Button>
        </Popconfirm>
      ),
    },
  ];

  return (
    <Panel.Default className={styles.Version}>
      <div className={styles.section}>
        <div className={styles.sectionTitle}>当前版本</div>
        <Descriptions
          column={2}
          items={[
            { label: "桌面端（随 hub 发版）", children: `v${desktop || "—"}` },
            { label: "移动端（APK）", children: android ? `v${android}` : "—" },
          ]}
        />
      </div>

      <div className={styles.section}>
        <div className={styles.sectionTitle}>强制更新下限</div>
        <div className={styles.hint}>
          低于下限的客户端/移动端必须更新后才能继续使用（拒绝即退出程序）。
          保存后写入下载目录 manifest.json，各端下一次心跳（约 2 秒）即生效。
        </div>
        <div className={styles.minRow}>
          <span className={styles.minLabel}>桌面端最低版本</span>
          <Input
            placeholder="如 0.3.0；留空为不强制"
            value={desktopMin}
            onChange={(v) => setDesktopMin(v)}
            className={styles.minInput}
          />
          <span className={styles.minLabel}>移动端最低版本</span>
          <Input
            placeholder="如 0.1.0；留空为不强制"
            value={androidMin}
            onChange={(v) => setAndroidMin(v)}
            className={styles.minInput}
          />
          <Button
            type="primary"
            loading={savingMin}
            onClick={saveMin}
            hasPermi={["sysmgmt:version:upd"]}
          >
            保存
          </Button>
        </div>
      </div>

      <div className={styles.section}>
        <div className={styles.sectionHead}>
          <div className={styles.sectionTitle}>更新日志</div>
          <Button
            type="primary"
            icon={<PlusOutlined />}
            onClick={() => setAddOpen(true)}
            hasPermi={["sysmgmt:version:upd"]}
          >
            新增
          </Button>
        </div>
        <Table<ChangelogEntry>
          rowKey="version"
          columns={columns}
          dataSource={changelog}
          loading={loading}
          pagination={false}
        />
      </div>

      <Modal
        title="新增更新日志"
        open={addOpen}
        onCancel={() => setAddOpen(false)}
        onOk={doAdd}
        okText="保存"
        cancelText="取消"
        confirmLoading={adding}
        centered
      >
        <div className={styles.formRow}>
          <div className={styles.formLabel}>版本号</div>
          <Input placeholder="如 0.3.1" value={addVersion} onChange={(v) => setAddVersion(v)} />
        </div>
        <div className={styles.formRow}>
          <div className={styles.formLabel}>更新说明（支持换行）</div>
          <Input.TextArea
            rows={5}
            placeholder="每行一条变更"
            value={addNotes}
            onChange={(v) => setAddNotes(v)}
          />
        </div>
      </Modal>
    </Panel.Default>
  );
};

export default Version;
