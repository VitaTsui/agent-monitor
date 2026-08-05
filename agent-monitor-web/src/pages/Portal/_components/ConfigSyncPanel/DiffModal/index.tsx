import React from "react";

import { Modal } from "@hsu-react/ui";

import styles from "./index.module.scss";

export interface DiffTarget {
  /** 字段名（hooks 会按事件/命令展开，其余按值对比） */
  field: string;
  /** 文件标识，如 claude/settings.json */
  file: string;
  /** 改动前 / 本机当前值 */
  from: unknown;
  /** 改动后 / 配置源上的值 */
  to: unknown;
  /** 弹窗标题里的设备名 */
  hostname?: string;
  /** 左右两栏的措辞：改动记录是「改动前/改动后」，待同步差异是「本机/配置源」 */
  fromLabel: string;
  toLabel: string;
}

interface Props {
  target: DiffTarget | null;
  onClose: () => void;
}

/** hooks 拍平后的一行：一条具体的命令 */
interface HookLine {
  key: string;
  event: string;
  matcher: string;
  command: string;
}

/**
 * hooks 拍平成「事件 + matcher + 命令」的列表。
 *
 * 不拍平就只能比对整棵树，用户看到的仍是「2 条 → 2 条」——
 * 真正想知道的是**哪一条命令**变了。
 */
function flattenHooks(v: unknown): HookLine[] {
  if (!v || typeof v !== "object") return [];
  const out: HookLine[] = [];
  for (const [event, entries] of Object.entries(v as Record<string, unknown>)) {
    if (!Array.isArray(entries)) continue;
    entries.forEach((e) => {
      const entry = e as Record<string, unknown> | null;
      const matcher = typeof entry?.matcher === "string" ? entry.matcher : "";
      const inner = entry?.hooks;
      if (!Array.isArray(inner)) return;
      inner.forEach((h) => {
        const hook = h as Record<string, unknown> | null;
        const command = typeof hook?.command === "string" ? hook.command : JSON.stringify(hook);
        out.push({ key: `${event}|${matcher}|${command}`, event, matcher, command });
      });
    });
  }
  return out;
}

type Mark = "same" | "added" | "removed";

/** 逐条比对：以「事件+matcher+命令」为准，改一条命令会显示成一删一增 */
function diffHooks(from: unknown, to: unknown): { line: HookLine; mark: Mark }[] {
  const a = flattenHooks(from);
  const b = flattenHooks(to);
  const bKeys = new Set(b.map((l) => l.key));
  const aKeys = new Set(a.map((l) => l.key));
  const rows: { line: HookLine; mark: Mark }[] = [];
  a.forEach((line) => rows.push({ line, mark: bKeys.has(line.key) ? "same" : "removed" }));
  b.forEach((line) => {
    if (!aKeys.has(line.key)) rows.push({ line, mark: "added" });
  });
  // 同一事件的行排在一起，读起来才像一份配置
  return rows.sort((x, y) => x.line.event.localeCompare(y.line.event));
}

/** 非 hooks 的值：直接给可读文本（对象转格式化 JSON） */
function asText(v: unknown): string {
  if (v === undefined || v === null) return "（无）";
  if (typeof v === "string") return v;
  return JSON.stringify(v, null, 2);
}

const MARK_TEXT: Record<Mark, string> = { same: " ", added: "+", removed: "-" };

/** 配置项改动的详情：hooks 展开到命令级，其余字段左右对照 */
const DiffModal: React.FC<Props> = ({ target, onClose }) => {
  if (!target) return null;
  const isHooks = target.field === "hooks";
  const rows = isHooks ? diffHooks(target.from, target.to) : [];
  const changed = rows.filter((r) => r.mark !== "same").length;

  return (
    <Modal
      className={styles.DiffModal}
      open={!!target}
      onCancel={onClose}
      footer={null}
      width={720}
      centered
      title={
        <div className={styles.head}>
          <span className={styles.headField}>{target.field}</span>
          <span className={styles.headMeta}>
            {target.file}
            {target.hostname ? ` · ${target.hostname}` : ""}
          </span>
        </div>
      }
    >
      {isHooks ? (
        <div className={styles.hookDiff}>
          {rows.length === 0 ? (
            <div className={styles.empty}>没有可展示的 hook</div>
          ) : (
            <>
              <div className={styles.summary}>
                {changed > 0 ? `${changed} 处改动` : "内容一致"}
                <span className={styles.legend}>
                  <em className={styles.removed}>- {target.fromLabel}</em>
                  <em className={styles.added}>+ {target.toLabel}</em>
                </span>
              </div>
              {rows.map(({ line, mark }, i) => (
                <div key={`${line.key}-${mark}-${i}`} className={`${styles.row} ${styles[mark]}`}>
                  <span className={styles.mark}>{MARK_TEXT[mark]}</span>
                  <span className={styles.event}>{line.event}</span>
                  {line.matcher ? <span className={styles.matcher}>{line.matcher}</span> : null}
                  <code className={styles.command}>{line.command}</code>
                </div>
              ))}
            </>
          )}
          {/* 同步进去的只有「通用」条目，界面上要讲清楚没列出来的那部分去哪了 */}
          <div className={styles.note}>
            只列出参与同步的「通用」条目。本客户端的配对 hook、以及命令指向本机路径的 hook
            不参与同步，也不会出现在这里 —— 它们始终留在各自的机器上。
          </div>
        </div>
      ) : (
        <div className={styles.valueDiff}>
          <div className={styles.side}>
            <div className={styles.sideTitle}>{target.fromLabel}</div>
            <pre className={styles.removed}>{asText(target.from)}</pre>
          </div>
          <div className={styles.side}>
            <div className={styles.sideTitle}>{target.toLabel}</div>
            <pre className={styles.added}>{asText(target.to)}</pre>
          </div>
        </div>
      )}
    </Modal>
  );
};

export default DiffModal;
