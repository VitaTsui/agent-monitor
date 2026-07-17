import React from "react";
import { CodeOutlined } from "@ant-design/icons";

import styles from "./index.module.scss";

/** 官网 Hero 的产品预览——纯静态 mock 演示数据，不含任何真实会话 */
const MockPortal: React.FC = () => {
  return (
    <div className={styles.MockPortal}>
      {/* 左侧栏 */}
      <div className={styles.sider}>
        <div className={styles.siderTop}>
          <span className={styles.logo}><CodeOutlined /></span>
          <span className={styles.name}>终端任务监控</span>
        </div>
        <div className={styles.search}>搜索会话 / 项目</div>
        <div className={styles.label}>设备</div>
        <div className={`${styles.device} ${styles.active}`}>
          <span>💻 MacBook Pro</span>
          <span className={styles.stat}>3 会话 · 1 执行中</span>
        </div>
        <div className={styles.device}>
          <span>🖥 Windows-PC</span>
          <span className={styles.stat}>2 会话</span>
        </div>
        <div className={styles.label}>会话</div>
        {[
          { d: "running", n: "重构支付模块的错误处理", p: "web-app · 执行中" },
          { d: "idle", n: "给用户表加索引", p: "api-server · 等待输入" },
          { d: "idle", n: "写单元测试覆盖登录流程", p: "web-app · 等待输入" },
        ].map((s) => (
          <div key={s.n} className={styles.sess}>
            <span className={`${styles.dot} ${styles[s.d]}`} />
            <div>
              <div className={styles.sessName}>{s.n}</div>
              <div className={styles.sessSub}>{s.p}</div>
            </div>
          </div>
        ))}
        <div className={styles.user}>
          <span className={styles.avatar}>超</span>
          <div>
            <div className={styles.uname}>超级管理员</div>
            <div className={styles.uplan}>超级管理员</div>
          </div>
        </div>
      </div>

      {/* 右侧对话区 */}
      <div className={styles.main}>
        <div className={styles.header}>
          <div className={styles.htitle}>重构支付模块的错误处理</div>
          <div className={styles.hmeta}>web-app · MacBook Pro · 执行中</div>
        </div>
        <div className={styles.chat}>
          <div className={styles.userBubble}>把支付回调里的异常处理重构一下，加上重试</div>
          <div className={styles.terminal}>
            <div className={styles.termHead}>
              <i /> <i /> <i /> <span>Claude Code</span>
            </div>
            <div className={styles.termBody}>
              <div className={styles.aLine}>
                <span className={styles.aDot}>⏺</span>
                <span>已定位到 3 处未捕获异常，开始加重试与退避…</span>
              </div>
              <div className={styles.tLine}>
                <span className={styles.tDot}>⏺</span>
                <span>Edit(src/payment/callback.ts)</span>
              </div>
              <div className={styles.rLine}>
                <span>⎿ 已更新 callback.ts（+24 -6）</span>
              </div>
              <div className={styles.tLine}>
                <span className={styles.tDot}>⏺</span>
                <span>Bash(npm test -- payment)</span>
              </div>
              <div className={styles.working}>
                <span className={styles.star}>✳</span> 执行中…
              </div>
            </div>
          </div>
        </div>
        <div className={styles.composer}>输入任务，回车发布</div>
      </div>
    </div>
  );
};

export default MockPortal;
