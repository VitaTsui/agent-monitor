/* eslint-disable @typescript-eslint/no-var-requires */
/**
 * 生成精简版 iconify 图标集（构建前自动执行，见 package.json 的 start / build）
 *
 * 做什么：扫 src 下所有 ts/tsx/js/jsx，把形如 `"ph:caret-down"` 的字面量捞出来，
 * 去 `@iconify/json` 的整集里查证，只把命中的那几枚裁进
 * `src/assets/iconify/collections.generated.json`，由 `src/index.tsx` 一次性
 * `addCollection` 注册。
 *
 * 为什么不手工维护子集（这脚本存在的理由）：
 * 1. 会漂 —— 源码里删掉某枚图标的用法，手工子集里那一枚不会自己消失，变成打进
 *    产物却没人用的死重量；反过来，写了 `ph:xxx` 却忘了往子集里补，运行时那枚图标
 *    直接空白，**而且不报错**，肉眼才能发现。
 * 2. 会撞 —— 并行改动时人人都改同一个 JSON，天天解冲突。生成物不入库就没得撞。
 *
 * 为什么不整集打进去：`ph.json` 单集就 4.3 MB，全量 `addCollection` 会整个进首屏。
 * 为什么不让运行时去 Iconify 公共 API 拉：这是要在内网/离线环境跑的监控工具，
 * 断网就是一片空白图标。注册过的名字 `@iconify/react` 一律走本地、不发请求。
 *
 * 用法：`pnpm gen:icons`（`pnpm start` / `pnpm build` 前会自动跑）
 */

const fs = require("fs");
const path = require("path");

/**
 * 参与扫描的图标集前缀白名单。
 *
 * 必须有白名单：`"a:b"` 这种字面量在代码里满地都是（时间 `"00:00"`、CSS 值、URL
 * 协议……），不限定前缀会捞回一堆假图标名。
 *
 * 目前只有 `ph`（Phosphor）—— 全项目经 `@iconify/react` 本地注册的就这一套。
 * 将来要引入别的集（如 `carbon`、`ep`、`fa-regular`），在这儿加一行即可，
 * 不用改 `src/index.tsx`：生成物是「集合数组」，注册那头是遍历。
 */
const PREFIXES = ["ph"];

/**
 * 额外保留清单 —— 扫描扫不到、但运行时确实要用的图标名，写全名（`prefix:name`）。
 *
 * 扫描是纯文本匹配，只认**写死在源码里的字面量**。以下两类看不见，必须登记在这里：
 *   a) 拼出来的名字，如 `` `ph:${kind}` ``；
 *   b) 从接口拿到的名字，如 `src/router/RouterService.tsx` 的
 *      `<Icon icon={item.icon} />`（后管菜单图标由 hub 下发）。
 *
 * 当前为空，因为：
 *   a) 全项目没有任何拼接写法 —— `ph:` 一律是字面量（`STATUS_ICON` / `CHAIN_ICON`
 *      映射表、JSX 里的三元 `expanded ? "ph:caret-up" : "ph:caret-down"`，都扫得到）；
 *   b) 后管菜单下发的是 `carbon:*`，不在本文件的 PREFIXES 里，属另一件事（见 README/报告）。
 *
 * 谁要写出上面那两类写法，就把图标全名补进这个数组，否则线上那枚图标是空白、
 * 且控制台一声不吭。
 */
const EXTRA_ICONS = [];

const ROOT = path.resolve(__dirname, "..");
const SRC_DIR = path.join(ROOT, "src");
const JSON_DIR = path.join(ROOT, "node_modules", "@iconify", "json", "json");
const OUT_DIR = path.join(SRC_DIR, "assets", "iconify");
const OUT_FILE = path.join(OUT_DIR, "collections.generated.json");

/** IconifyJSON 顶层可选的默认属性，要一并带上，否则图标尺寸会错 */
const ROOT_KEYS = ["width", "height", "left", "top", "rotate", "hFlip", "vFlip"];

function walk(dir, acc = []) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      // assets 里就是生成物自己，扫它等于自我循环
      if (entry.name === "node_modules" || entry.name === "assets") continue;
      walk(full, acc);
    } else if (/\.(tsx?|jsx?)$/.test(entry.name)) {
      acc.push(full);
    }
  }
  return acc;
}

/**
 * 扫源码收集候选图标名。
 *
 * 纯文本匹配、不区分注释与代码：注释里写成 `` `ph:caret-down` `` 的举例也会被当真
 * 收进来。这是刻意接受的 —— 想精确就得上 AST，而本项目注释里提到的图标名恰恰都是
 * 代码里真在用的那几枚，体积代价为零；就算多收一枚也不过几百字节。
 */
function collectCandidates() {
  const re = /["'`]([a-z0-9]+(?:-[a-z0-9]+)*):([a-z0-9]+(?:-[a-z0-9]+)*)["'`]/g;
  const byPrefix = new Map();

  for (const file of walk(SRC_DIR)) {
    const code = fs.readFileSync(file, "utf8");
    let m;
    while ((m = re.exec(code))) {
      const [, prefix, name] = m;
      if (!PREFIXES.includes(prefix)) continue;
      if (!byPrefix.has(prefix)) byPrefix.set(prefix, new Set());
      byPrefix.get(prefix).add(name);
    }
  }

  return byPrefix;
}

/** 把额外保留清单并进候选集，返回它们的全名集合（漏掉时要单独报警） */
function mergeExtraIcons(byPrefix) {
  const extras = new Set();

  for (const full of EXTRA_ICONS) {
    const idx = full.indexOf(":");
    if (idx <= 0) {
      console.warn(`[gen:icons] EXTRA_ICONS 里 "${full}" 不是 prefix:name 形式，已忽略`);
      continue;
    }

    const prefix = full.slice(0, idx);
    const name = full.slice(idx + 1);
    if (!byPrefix.has(prefix)) byPrefix.set(prefix, new Set());
    byPrefix.get(prefix).add(name);
    extras.add(full);
  }

  return extras;
}

/** 把一枚图标（可能是 alias）连同它的 parent 链一起收进结果 */
function resolveIcon(source, target, name, seen = new Set()) {
  if (seen.has(name)) return false; // 防御 alias 成环
  seen.add(name);

  if (source.icons?.[name]) {
    target.icons[name] = source.icons[name];
    return true;
  }

  const alias = source.aliases?.[name];
  if (alias) {
    target.aliases = target.aliases || {};
    target.aliases[name] = alias;
    return alias.parent ? resolveIcon(source, target, alias.parent, seen) : true;
  }

  return false;
}

function main() {
  if (!fs.existsSync(JSON_DIR)) {
    console.error(
      `[gen:icons] 找不到 ${path.relative(ROOT, JSON_DIR)}，请先装依赖（@iconify/json 是 devDependency）`
    );
    process.exit(1);
  }

  const candidates = collectCandidates();
  const extras = mergeExtraIcons(candidates);

  const collections = [];
  const missing = [];
  const missingExtras = [];
  let total = 0;

  // EXTRA_ICONS 可能带来白名单之外的集合，所以取并集而不是只走 PREFIXES
  const prefixes = [...new Set([...PREFIXES, ...candidates.keys()])];

  for (const prefix of prefixes) {
    const names = candidates.get(prefix);
    if (!names || names.size === 0) continue;

    const file = path.join(JSON_DIR, `${prefix}.json`);
    if (!fs.existsSync(file)) {
      console.warn(`[gen:icons] 找不到图标集 ${prefix}.json，跳过`);
      continue;
    }

    const source = JSON.parse(fs.readFileSync(file, "utf8"));
    const target = { prefix, icons: {} };
    for (const key of ROOT_KEYS) {
      if (source[key] !== undefined) target[key] = source[key];
    }

    let hit = 0;
    for (const name of [...names].sort()) {
      const full = `${prefix}:${name}`;
      if (resolveIcon(source, target, name)) {
        hit += 1;
      } else if (extras.has(full)) {
        missingExtras.push(full);
      } else {
        missing.push(full);
      }
    }

    if (hit > 0) {
      collections.push(target);
      total += hit;
    }
  }

  fs.mkdirSync(OUT_DIR, { recursive: true });
  fs.writeFileSync(OUT_FILE, JSON.stringify(collections), "utf8");

  const size = fs.statSync(OUT_FILE).size;
  console.log(
    `[gen:icons] 已生成 ${collections.length} 个精简图标集、${total} 枚图标，共 ${size} B（${(size / 1024).toFixed(1)} KB）`
  );
  if (missingExtras.length) {
    // 登记了却查无此图标：线上就是一枚空白图标，必须改掉登记
    console.warn(
      `[gen:icons] EXTRA_ICONS 里这些图标在图标集里不存在，页面上会是空白：\n  ${missingExtras.join("\n  ")}`
    );
  }
  if (missing.length) {
    console.log(
      `[gen:icons] 以下字面量像图标名但集合里查无此图标，已忽略：\n  ${missing.join("\n  ")}`
    );
  }
}

main();
