import React from "react";

import { Button } from "@hsu-react/ui";
import { useNavigate } from "react-router-dom";
import {
  AndroidFilled,
  AppleFilled,
  CloudServerOutlined,
  CodeOutlined,
  ControlOutlined,
  DesktopOutlined,
  EyeOutlined,
  LockOutlined,
  SafetyCertificateOutlined,
  ThunderboltFilled,
  WindowsFilled,
} from "@ant-design/icons";

import { getAccessToken } from "@/utils/auth";
import MockPortal from "./_components/MockPortal";
import styles from "./index.module.scss";

const FEATURES = [
  {
    icon: <EyeOutlined />,
    title: "实时会话监控",
    desc: "解析 Claude Code / Codex 会话，WebSocket 实时推送提示词、工具调用、任务清单与后台任务；其余代理进程级接管，同样可控。",
  },
  {
    icon: <ControlOutlined />,
    title: "远程控制与发布",
    desc: "暂停 / 恢复 / 中断 / 终止正在运行的代理任务，或直接向会话注入一行输入发布新任务——就在网页上。",
  },
  {
    icon: <CloudServerOutlined />,
    title: "多机聚合",
    desc: "Mac / Windows / Linux 多台电脑的会话统一聚合到一处，按设备与终端类型分组，一屏总览全部代理动态。",
  },
  {
    icon: <LockOutlined />,
    title: "隐私隔离",
    desc: "只能看到自己名下、且已信任的设备；会话内容纯实时读取、我方不落存储，超级管理员也看不到别人的会话。",
  },
  {
    icon: <SafetyCertificateOutlined />,
    title: "安全加固",
    desc: "口令 RSA+AES 加密传输、加盐哈希存储，后管部署令牌双重锁，危险指令发布需多重确认。",
  },
  {
    icon: <DesktopOutlined />,
    title: "桌面 & 移动端",
    desc: "Mac / Windows 桌面应用内嵌完整前台，关闭可缩到托盘后台同步、开机自启；Android 应用随时随地查看与控制。",
  },
];

const STEPS = [
  { n: "1", t: "服务端部署", d: "在一台服务器上运行 hub，自动聚合各机数据、托管网页界面。" },
  { n: "2", t: "客户端接入", d: "每台电脑安装客户端并登录账号，本机自动绑定、安全上报会话。" },
  { n: "3", t: "网页监控", d: "登录网页前台，实时查看、控制、发布任务；信任设备后即可见其会话。" },
];

/** 客户端安装包直链（hub /downloads 托管；开发经 /api 代理） */
const DL_BASE = `${process.env.API_BASE ?? ""}/downloads`;
const DOWNLOADS = {
  mac: `${DL_BASE}/${encodeURIComponent("终端任务监控.dmg")}`,
  win: `${DL_BASE}/${encodeURIComponent("终端任务监控.exe")}`,
  android: `${DL_BASE}/${encodeURIComponent("终端任务监控.apk")}`,
};

const Home: React.FC = () => {
  const navigate = useNavigate();
  const loggedIn = !!getAccessToken();
  const enter = () => navigate(loggedIn ? "/portal" : "/login?redirect=%2Fportal");

  return (
    <div className={styles.Home}>
      {/* 顶部导航 */}
      <header className={styles.nav}>
        <div className={styles.navInner}>
          <div className={styles.brand}>
            <span className={styles.logo}><CodeOutlined /></span>
            <span className={styles.brandName}>终端任务监控</span>
          </div>
          <nav className={styles.navLinks}>
            <a href="#features">特性</a>
            <a href="#how">工作原理</a>
            <a href="#clients">下载</a>
          </nav>
          <Button className={styles.navCta} type="primary" onClick={enter}>
            {loggedIn ? "进入前台" : "登录"}
          </Button>
        </div>
      </header>

      {/* Hero */}
      <section className={styles.hero}>
        <div className={styles.heroText}>
          <div className={styles.badge}>
            <ThunderboltFilled /> 面向所有 AI 编码代理的终端监控
          </div>
          <h1 className={styles.title}>
            盯住每一台电脑上
            <br />
            正在跑的 <span className={styles.accent}>AI 编码代理</span>
          </h1>
          <p className={styles.subtitle}>
            一处网页，实时监控多台 Mac / Windows / Linux 终端里正在执行的
            Claude Code、Codex、Gemini CLI 等 AI 编码代理任务，随时暂停、中断、
            注入新指令。纯实时、不落存储，按设备归属严格隔离。
          </p>
          <div className={styles.heroActions}>
            <Button className={styles.primaryBtn} type="primary" onClick={enter}>
              {loggedIn ? "进入前台" : "免费开始"}
            </Button>
            <a className={styles.ghostBtn} href="#clients">
              下载客户端
            </a>
          </div>
          <div className={styles.heroMeta}>
            <span>无需信用卡</span>
            <span>·</span>
            <span>支持自助注册</span>
            <span>·</span>
            <span>Google / Apple 登录</span>
          </div>
        </div>
        <div className={styles.heroPreview}>
          {/* 产品预览：全部为 mock 演示数据 */}
          <MockPortal />
          <div className={styles.previewNote}>产品界面预览 · 演示数据</div>
        </div>
      </section>

      {/* 特性 */}
      <section id="features" className={styles.features}>
        <div className={styles.sectionHead}>
          <h2>为「看住 AI 代理」而生</h2>
          <p>从会话解析到远程控制，一套完整的终端代理监控能力。</p>
        </div>
        <div className={styles.featureGrid}>
          {FEATURES.map((f) => (
            <div key={f.title} className={styles.featureCard}>
              <span className={styles.featureIcon}>{f.icon}</span>
              <div className={styles.featureTitle}>{f.title}</div>
              <div className={styles.featureDesc}>{f.desc}</div>
            </div>
          ))}
        </div>
      </section>

      {/* 工作原理 */}
      <section id="how" className={styles.how}>
        <div className={styles.sectionHead}>
          <h2>三步接入</h2>
          <p>服务端 + 客户端 + 网页，十分钟跑起来。</p>
        </div>
        <div className={styles.steps}>
          {STEPS.map((s) => (
            <div key={s.n} className={styles.step}>
              <span className={styles.stepNum}>{s.n}</span>
              <div className={styles.stepTitle}>{s.t}</div>
              <div className={styles.stepDesc}>{s.d}</div>
            </div>
          ))}
        </div>
      </section>

      {/* 下载客户端 */}
      <section id="clients" className={styles.clients}>
        <div className={styles.sectionHead}>
          <h2>客户端</h2>
          <p>
            桌面端是正常应用程序：打开即完整前台，关闭可选缩小到系统托盘——
            后台持续同步本机会话，也可开机自启。
          </p>
        </div>
        <div className={styles.clientCards}>
          <a className={styles.clientCard} href={DOWNLOADS.mac} download>
            <AppleFilled className={styles.clientIcon} />
            <div className={styles.clientName}>macOS</div>
            <div className={styles.clientDesc}>
              通用版（Apple 芯片 / Intel），完整前台 + 托盘后台同步
            </div>
            <span className={styles.clientDl}>下载 .dmg</span>
          </a>
          <a className={styles.clientCard} href={DOWNLOADS.win} download>
            <WindowsFilled className={styles.clientIcon} />
            <div className={styles.clientName}>Windows</div>
            <div className={styles.clientDesc}>单文件程序，双击即用，完整前台 + 托盘后台同步</div>
            <span className={styles.clientDl}>下载 .exe</span>
          </a>
          <a className={styles.clientCard} href={DOWNLOADS.android} download>
            <AndroidFilled className={styles.clientIcon} />
            <div className={styles.clientName}>Android</div>
            <div className={styles.clientDesc}>移动端应用，前台功能随时随地可用</div>
            <span className={styles.clientDl}>下载 .apk</span>
          </a>
        </div>
        <div className={styles.clientHint}>
          macOS：打开 dmg 后<b>双击「安装.command」</b>一键装好并启动
          （若被拦：右键它 →「打开」）。
          安装后在客户端里登录你的账号，本机即自动绑定并建立链接——
          网页、移动端与其他客户端上立刻可见这台电脑的终端会话（设备管理里可随时断开）。
        </div>
      </section>

      {/* CTA */}
      <section className={styles.cta}>
        <h2>现在就看住你的 AI 代理</h2>
        <Button className={styles.primaryBtn} type="primary" onClick={enter}>
          {loggedIn ? "进入前台" : "登录 / 注册"}
        </Button>
      </section>

      <footer className={styles.footer}>
        <div className={styles.footBrand}>
          <span className={styles.logo}><CodeOutlined /></span>
          <span>终端任务监控</span>
        </div>
        <div className={styles.footNote}>
          终端 AI 代理任务监控平台 · 纯实时不落存储 · © {new Date().getFullYear()}
        </div>
      </footer>
    </div>
  );
};

export default Home;
