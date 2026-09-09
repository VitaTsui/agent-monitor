#!/usr/bin/env bash
# 生成 .env/.env.prod —— 生产构建（pnpm build）唯一要读的环境文件。
#
# 为什么需要这个脚本：.env.prod 里的 CRYPTO_KEY / RSA_PUB_KEY 必须与线上 hub 的
# AM_CRYPTO_KEY / RSA 私钥配对，属于「部署方持有的配置」，不进版本库；而
# config/webpack.config.prod.cjs 又必须读到这个文件才编得出包。于是结构留在
# .env/.env.prod.example（入库），两个配对值由环境变量注入（CI 走仓库变量）。
#
# 用法:
#   CRYPTO_KEY=xxx RSA_PUB_KEY=yyy bash scripts/write-env-prod.sh
set -euo pipefail

cd "$(dirname "$0")/.."

TPL=".env/.env.prod.example"
OUT=".env/.env.prod"

: "${CRYPTO_KEY:?缺 CRYPTO_KEY —— CI 取自仓库变量 vars.WEB_CRYPTO_KEY，本地见 $TPL 顶部说明}"
: "${RSA_PUB_KEY:?缺 RSA_PUB_KEY —— CI 取自仓库变量 vars.WEB_RSA_PUB_KEY，本地见 $TPL 顶部说明}"

# 用 awk 整行替换而不是 sed 就地替换：这两个值是 base64/PEM，里面有 / 和 +，
# 走 sed 的替换串会被当成分隔符和反向引用。
awk -v ck="$CRYPTO_KEY" -v rk="$RSA_PUB_KEY" '
  /^CRYPTO_KEY=/  { print "CRYPTO_KEY=" ck;   next }
  /^RSA_PUB_KEY=/ { print "RSA_PUB_KEY=" rk;  next }
                  { print }
' "$TPL" > "$OUT"

# 模板里的占位符必须全部被换掉，否则会编出一个登录必然失败的包。
if grep -q '__CRYPTO_KEY__\|__RSA_PUB_KEY__' "$OUT"; then
  echo "✗ $OUT 仍有未替换的占位符，检查 $TPL 里的键名是否被改过" >&2
  exit 1
fi

echo "✓ 已生成 $OUT"
