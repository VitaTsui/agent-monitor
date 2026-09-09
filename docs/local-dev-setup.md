# 本地联调环境搭建

在本机把 hub + 前端跑起来、造出可操作的会话，用于验证改动。
每一条都是实际踩过的坑 —— 少一步就卡住，而报错信息往往不指向真正的原因。

## 一、密钥：不用手造，hub 首启自己生成

hub 的 RSA 私钥是**运行时**读文件的（`hub/src/main.rs` 的 `load_or_create_rsa_key`），
不再编译期 `include_str!` 嵌入 —— 干净 clone 直接 `cargo build` 就能过，不需要预先造密钥。

首次启动时，若 `AM_RSA_KEY_PATH`（缺省 `<AM_DATA_DIR>/rsa_private.pem`）指的文件不存在，
hub 会现生成一对 2048 位密钥落盘（0600），并在日志里打印配对公钥的 base64；
AES 密钥同理，`AM_CRYPTO_KEY` 不设就生成到 `<AM_DATA_DIR>/crypto-key` 并打印。

`.env.dev` 不入库，先 `cp agent-monitor-web/.env/.env.dev.example agent-monitor-web/.env/.env.dev`。

**本地要走真口令登录**，就把这两个打印值填进 `agent-monitor-web/.env/.env.dev` 的
`RSA_PUB_KEY` / `CRYPTO_KEY` 再重启前端 —— 两边不配对时登录会「无请求即失败」
（浏览器里加密就崩了，网络面板一条请求都没有）。反过来也行：先有 `.env.dev` 的值，
就用 `AM_CRYPTO_KEY=<.env.dev 里的值>` 起 hub，并把配对私钥放到 `AM_RSA_KEY_PATH`。

## 二、起 hub

```bash
AM_PORT=8383 AM_DATA_DIR=/tmp/am-dev cargo run -q -p am-hub
```

前端 dev server 的代理写死指向 `localhost:8383`，端口别改。

## 三、跳过登录（关键坑一：token 的存储格式）

前端用 `web-storage-cache` 存 token，**不是裸字符串**。直接
`localStorage.setItem("FZXVM_ACCESS_TOKEN", "xxx")` 会被判为无效、踢回登录页，
且没有任何提示。正确格式是 `{c: 创建时间, e: 过期时间, v: JSON序列化后的值}`：

先在 hub 数据目录预置一个会话 token：

```bash
NOW=$(date +%s)
cat > /tmp/am-dev/sessions.json <<EOF
{"devtoken":{"username":"admin","last_seen":$NOW}}
EOF
```

再在浏览器里写入（注意 `v` 是**二次** JSON 序列化的）：

```js
localStorage.setItem("FZXVM_ACCESS_TOKEN", JSON.stringify({
  c: Date.now(),
  e: 253402300799000,
  v: JSON.stringify("devtoken"),
}));
```

排查技巧：先看 `localStorage` 里已有的 `lang` 长什么样，照着它的格式写就不会错。

## 四、造一个可操作的会话（关键坑二：owner + 心跳）

只上报一次是不够的，有两道门槛：

**1. 设备必须归属某个账号**。`can_view` 要求「已信任 **且** 已归属」，而上报建立的设备
`owner` 是 `null` —— 表现为网页显示「暂无设备」，但 `/monitor/tasks` 用 token 直接查却有数据。

```bash
# 上报一次让设备条目产生，然后补 owner，重启 hub 生效
sed -i 's/"owner": null,/"owner": "admin",/' /tmp/am-dev/registry.json
```

**2. 必须持续上报心跳**。`OFFLINE_AFTER_SECS = 10`，超过 10 秒没上报即判离线，
设备会从列表里消失。所以要起个循环：

```bash
TOKEN=$(cat /tmp/am-dev/agent-token)   # hub 首次启动时生成
cat > /tmp/report.json <<'JSON'
{"machineId":"dev-machine","hostname":"DEV-PC","platform":"windows","version":"0.8.6",
 "tasks":[{"id":"dev-1","provider":"claude","project":"D:/your/project","projectName":"your-project",
 "title":"dev session","prompt":"","lastAction":"","status":"idle","statusDsr":"waiting",
 "providerDsr":"Claude Code","ideDsr":"Cursor","pid":12345,"machineId":"dev-machine",
 "hostname":"DEV-PC","platform":"windows","platformDsr":"Windows","startedAt":null,
 "lastActiveAt":null,"mtimeMs":0,"lineCount":10,"version":null,"gitBranch":null,"process":null}]}
JSON

while true; do
  curl -s -o /dev/null -X POST http://127.0.0.1:8383/monitor/report \
    -H 'Content-Type: application/json' -H "x-agent-token: $TOKEN" \
    --data-binary @/tmp/report.json
  sleep 3
done &
```

**payload 里的中文要走文件**：直接写在 `curl -d '...'` 里会被 shell 处理坏，
hub 报 `invalid unicode code point`。用 `--data-binary @文件` 就没事。

## 五、起前端

```bash
cd agent-monitor-web && yarn start     # http://localhost:3004
```

## 六、常见现象对照

| 现象 | 原因 |
|---|---|
| 登录点了没反应、网络面板无请求 | 前端 `.env.dev` 的 CRYPTO_KEY/RSA_PUB_KEY 与 hub 不配对，见第一节 |
| 访问 /portal 被弹回 /login | token 格式不对（不是裸字符串），见第三节 |
| 网页「暂无设备」但 API 查得到数据 | 设备 `owner` 为 null，见第四节 |
| 设备出现几秒后消失 | 没有持续心跳，10 秒判离线 |
| 上报报 `invalid unicode code point` | 中文 payload 写在命令行里了，改用 `--data-binary @file` |

## 七、收尾

```bash
pkill -f "while true; do curl"   # 心跳
pkill -f "vite"                 # 前端
rm -rf /tmp/am-dev /tmp/report.json   # 数据目录里含首启生成的密钥，一并清掉
```
