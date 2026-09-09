# agent-monitor 部署指南（monitor.vita-llm.com）

服务器 `<PROD_HOST>`（Ubuntu 24.04），域名 `monitor.vita-llm.com`。

> 本文档中的 `<PROD_HOST>`（服务器 IP / 主机名）与 `<SSH_USER>`（SSH 登录用户）
> 是占位符，**不入版本库**。真实值由部署者自持：`<PROD_HOST>` 见你的 VPS 控制台
> 或本地 `~/.ssh/config`，`<SSH_USER>` 见同一处；密钥类真实值见本地 `deploy/env`
> （从 `deploy/env.example` 复制后填写）。

本目录含 Caddy/systemd 配置、一键安装脚本，以及部署所需的交叉编译二进制、
生产密钥、前端构建。

> ⚠️ 二进制、前端产物（`agent-task-monitor`、`web/`）与全部密钥（`env`、
> `rsa_private.pem`、各令牌 `.txt`）**不入版本库**（见 `.gitignore`）——它们只存在于
> 本地工作副本，从本地 rsync 到服务器即可。若从全新 clone 起步：二进制用
> `cargo zigbuild -p am-hub --release --target x86_64-unknown-linux-musl`
> 重新交叉编译，前端 `yarn build`，`env` 从 `env.example` 复制后填入真实密钥。

## 步骤 0 · 先配 DNS（务必先做）

到 vita-llm.com 的 DNS 管理，加一条 A 记录：

```
类型 A   主机 monitor   值 <PROD_HOST>   TTL 默认
```

`dig monitor.vita-llm.com +short` 能解析到 <PROD_HOST> 后再继续（否则 Caddy 签证书会失败）。

## 步骤 1 · 上传部署目录到服务器

在本机（Mac）项目根执行：

```bash
# 只传部署必需文件（排除 .txt 凭据与本地备份，凭据你自己另存 CREDENTIALS.txt）
rsync -avz --exclude '*.txt' --exclude '*.bak' \
  deploy/ <SSH_USER>@<PROD_HOST>:/root/agent-monitor-deploy/
```

或用 scp：`scp -r deploy <SSH_USER>@<PROD_HOST>:/root/agent-monitor-deploy`

## 步骤 2 · 服务器上一键安装

```bash
ssh <SSH_USER>@<PROD_HOST>
cd /root/agent-monitor-deploy
sudo bash install.sh
```

脚本会自动：建低权限用户 `agentmon` → 装程序/前端/密钥到 `/opt/agent-monitor` →
装并启动 systemd 服务 → 安装 Caddy 并配好反代 + 自动 HTTPS → 配防火墙（放行
22/80/443，封锁公网直连 8383）。

首次 HTTPS 证书签发约几十秒。完成后访问：

- 前台：<https://monitor.vita-llm.com/portal>
- 后管：<https://monitor.vita-llm.com/admin>（登录后输入后管访问令牌解锁）

凭据见 `CREDENTIALS.txt`。

## 步骤 3 · 让其它电脑接入（agent 模式）

在每台要监控的电脑上跑 agent 版（该机需已装并使用过 Claude Code）。二进制按平台自备：
Mac/Linux 用本仓库 `cargo build -p am-hub --release` 产物（bin 名仍为 agent-task-monitor）。

macOS / Linux：

```bash
AM_HUB_URL=https://monitor.vita-llm.com \
AM_AGENT_TOKEN=<CREDENTIALS.txt 里的 agent 上报令牌> \
AM_USER=<你的登录用户名> \
./agent-task-monitor
```

Windows (PowerShell)：

```powershell
$env:AM_HUB_URL="https://monitor.vita-llm.com"
$env:AM_AGENT_TOKEN="<agent 上报令牌>"
$env:AM_USER="<你的登录用户名>"
.\agent-task-monitor.exe
```

接入后设备处于「待信任」，到前台「设备管理」里信任它，其会话才会显示。
`AM_USER` 决定设备归属哪个账号——每个用户只看得到自己名下且已信任的设备。

## 运维速查

```bash
systemctl status agent-monitor          # 服务状态
journalctl -u agent-monitor -f          # 实时日志
systemctl restart agent-monitor         # 重启
cat /opt/agent-monitor/data/registry.json   # 用户/设备注册表（唯一需备份的状态）
```

**要备份的只有** `/opt/agent-monitor/data/`（用户账号 + 设备信任 + 额度）和
`/opt/agent-monitor/env` + `rsa_private.pem`（密钥）。几个 KB，cron 拷走即可。

## 更新版本（以后改了代码）

1. 本机重新交叉编译：`cd agent-task-monitor && cargo zigbuild -p am-hub --release --target x86_64-unknown-linux-musl`
2. 前端（若改了）：用生产密钥的 `.env.prod` 跑 `yarn build`
3. 传新的 `agent-task-monitor` 和 `web/` 覆盖 `/opt/agent-monitor/` 下同名文件（属主改回 agentmon）
4. `systemctl restart agent-monitor`

密钥/数据不动，无需重新配置。
