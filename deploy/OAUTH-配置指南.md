# Google / Apple 登录配置指南

配好凭据、在服务器 `env` 里填上环境变量并重启服务，登录页的对应按钮就会自动激活。

**服务端固定信息（填表要用）**
- 域名 / 网站 URL：`https://monitor.vita-llm.com`
- Google 回调（Authorized redirect URI）：`https://monitor.vita-llm.com/login`
- Apple 回调（Return URL）：`https://monitor.vita-llm.com/auth/access/oauth/apple/callback`

---

## 一、Google 登录（约 5 分钟，免费）

1. 打开 **Google Cloud Console** → <https://console.cloud.google.com/>
2. 顶部创建一个项目（或选已有项目）。
3. 左侧 **APIs & Services → OAuth consent screen（OAuth 同意屏幕）**：
   - User Type 选 **External（外部）**，填应用名「终端任务监控」、支持邮箱、开发者邮箱，保存。
   - 无需提交审核即可给自己/测试用户用；要对所有人开放再点「发布应用」。
4. 左侧 **APIs & Services → Credentials（凭据）→ Create Credentials → OAuth client ID**：
   - Application type：**Web application**
   - Name：随意，如 `agent-monitor-web`
   - **Authorized redirect URIs** 添加一条：
     ```
     https://monitor.vita-llm.com/login
     ```
   - 创建后得到 **Client ID** 和 **Client secret**，复制下来。
5. 把两个值填到服务器 `/opt/agent-monitor/env`：
   ```
   AM_PUBLIC_URL=https://monitor.vita-llm.com
   AM_GOOGLE_CLIENT_ID=<你的 Client ID>
   AM_GOOGLE_CLIENT_SECRET=<你的 Client secret>
   ```
6. 重启：`systemctl restart agent-monitor`。刷新登录页，Google 按钮即可用。

> `AM_PUBLIC_URL` 是两个渠道共用的开关，必须填，否则按钮保持禁用。

---

## 二、Apple 登录（较繁琐，需**付费** Apple Developer 账号 $99/年）

Apple Sign In 只有付费开发者账号能用。步骤在 <https://developer.apple.com/account> 完成：

### 1. 建 App ID（Identifier）
Certificates, Identifiers & Profiles → **Identifiers** → ➕ → **App IDs** → App
- Description 随意，Bundle ID 如 `com.vitahsu.agentmonitor`
- 勾选 **Sign In with Apple**，保存。

### 2. 建 Services ID（这就是网页登录用的 client_id）
Identifiers → ➕ → **Services IDs**
- Description：如 `agent-monitor-web`
- Identifier：如 `com.vitahsu.agentmonitor.web` ← **这个字符串就是 `AM_APPLE_CLIENT_ID`**
- 保存后点进去，勾选 **Sign In with Apple → Configure**：
  - Primary App ID：选上一步的 App ID
  - **Domains and Subdomains**：`monitor.vita-llm.com`
  - **Return URLs**：
    ```
    https://monitor.vita-llm.com/auth/access/oauth/apple/callback
    ```
  - 保存。

### 3. 建密钥（.p8）
Keys → ➕ → 勾选 **Sign In with Apple** → Configure 选 Primary App ID → 保存下载 **AuthKey_XXXX.p8**（只能下一次，存好）。
记下三个值：
- **Key ID**（这个 Key 的 ID，10 位）
- **Team ID**（右上角账号信息里，10 位）
- **Services ID**（第 2 步那个 `com.vitahsu.agentmonitor.web`）

### 4. 用 .p8 生成 client_secret（一段 JWT，最长 6 个月有效）
Apple 的 client_secret 不是固定字符串，是用 .p8 私钥签的 ES256 JWT。本地用脚本生成一次：

```bash
# 需要：pip install pyjwt cryptography
python3 - <<'PY'
import jwt, time
TEAM_ID = "你的 Team ID"
KEY_ID  = "你的 Key ID"
CLIENT_ID = "com.vitahsu.agentmonitor.web"   # Services ID
PRIVATE_KEY = open("AuthKey_XXXX.p8").read()
now = int(time.time())
token = jwt.encode(
    {"iss": TEAM_ID, "iat": now, "exp": now + 15552000,  # 180 天
     "aud": "https://appleid.apple.com", "sub": CLIENT_ID},
    PRIVATE_KEY, algorithm="ES256", headers={"kid": KEY_ID})
print(token)
PY
```

把输出的一长串 JWT 作为 `AM_APPLE_CLIENT_SECRET`。

### 5. 填服务器 `env` 并重启
```
AM_PUBLIC_URL=https://monitor.vita-llm.com
AM_APPLE_CLIENT_ID=com.vitahsu.agentmonitor.web
AM_APPLE_CLIENT_SECRET=<上一步生成的 JWT>
```
`systemctl restart agent-monitor` → 刷新登录页，Apple 按钮即可用。

> ⚠️ Apple 的 client_secret 最长 180 天，到期需按第 4 步重新生成一次并更新 env。
> 这是 Apple 的硬性限制；可以写个 cron 每 5 个月自动重签。

---

## 验证

配好后到 <https://monitor.vita-llm.com/login> 点对应按钮：
- 跳转到 Google/Apple 授权页 → 授权 → 自动回跳并登录成功。
- 首次用第三方账号登录会**自动注册**一个以邮箱为用户名的账号（普通用户）。

## 只想开一个？

两个渠道相互独立，可只配 Google（简单免费）先用起来，Apple 以后再说。
Google 的 `AM_GOOGLE_*` 填了 Google 按钮就亮；Apple 的填了 Apple 按钮才亮。
