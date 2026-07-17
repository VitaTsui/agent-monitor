#!/usr/bin/env bash
# 在 Ubuntu 24.04 服务器上一键安装 agent-task-monitor (hub) + Caddy。
# 用法：把整个 deploy/ 目录上传到服务器，然后 sudo bash install.sh
set -euo pipefail

DOMAIN="monitor.vita-llm.com"
APP_DIR="/opt/agent-monitor"
SRC_DIR="$(cd "$(dirname "$0")" && pwd)"

echo "==> 1/7 创建运行用户与目录"
id -u agentmon &>/dev/null || useradd --system --no-create-home --shell /usr/sbin/nologin agentmon
mkdir -p "$APP_DIR/data" "$APP_DIR/web"

echo "==> 2/7 拷贝程序、前端、密钥、配置"
install -m 0755 "$SRC_DIR/agent-task-monitor" "$APP_DIR/agent-task-monitor"
rm -rf "$APP_DIR/web"; cp -R "$SRC_DIR/web" "$APP_DIR/web"
install -m 0600 "$SRC_DIR/rsa_private.pem" "$APP_DIR/rsa_private.pem"
install -m 0600 "$SRC_DIR/env" "$APP_DIR/env"
chown -R agentmon:agentmon "$APP_DIR"

echo "==> 3/7 安装 systemd 服务"
install -m 0644 "$SRC_DIR/agent-monitor.service" /etc/systemd/system/agent-monitor.service
systemctl daemon-reload
systemctl enable agent-monitor
systemctl restart agent-monitor

echo "==> 4/7 安装 Caddy（若未安装）"
if ! command -v caddy &>/dev/null; then
  apt-get update -y
  apt-get install -y debian-keyring debian-archive-keyring apt-transport-https curl
  curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
  curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' > /etc/apt/sources.list.d/caddy-stable.list
  apt-get update -y
  apt-get install -y caddy
fi

echo "==> 5/7 配置 Caddy 反向代理 + 自动 HTTPS"
install -m 0644 "$SRC_DIR/Caddyfile" /etc/caddy/Caddyfile
systemctl restart caddy

echo "==> 6/7 配置防火墙（放行 22/80/443，封锁直连 8383）"
if command -v ufw &>/dev/null; then
  ufw allow 22/tcp   || true
  ufw allow 80/tcp   || true
  ufw allow 443/tcp  || true
  ufw deny 8383/tcp  || true
  yes | ufw enable   || true
fi

echo "==> 7/7 完成。状态："
systemctl --no-pager status agent-monitor | head -5 || true
echo
echo "访问： https://$DOMAIN/portal （前台）  https://$DOMAIN/admin （后管）"
echo "首次 HTTPS 证书签发需几十秒；确认 $DOMAIN 的 A 记录已指向本机公网 IP。"
echo "查看日志： journalctl -u agent-monitor -f"
