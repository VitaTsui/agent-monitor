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
 * 扫 `src` 时的图标集前缀白名单。
 *
 * 这一路必须有白名单：`"a:b"` 这种字面量在自己写的代码里满地都是（时间 `"00:00"`、
 * CSS 值、i18n key……），不限定前缀会捞回一堆假图标名，也没法再区分「拼错了图标名」
 * 和「本来就不是图标名」—— 而前者正是要报出来的。
 *
 * 用到新图标集时在这儿加一行即可，不用改 `src/index.tsx`：生成物是「集合数组」，
 * 注册那头是遍历。
 */
const PREFIXES = ["ph", "fa-regular", "ep"];

/**
 * 额外扫描的**依赖**目录 —— 只列自身写死了 iconify 图标名的库。
 *
 * 为什么必须扫库：`@iconify/react` 的规矩是「没 `addCollection` 过的名字就去
 * `api.iconify.design` 拉」，它不管这名字是谁写的。`@hsu-react/ui` 在自己组件里
 * 写死了几十枚（Select 的下拉箭头 `ep:arrow-down`、Table 翻页、Copy、Tree 搜索、
 * Chat…… 见 `es/components/**`），这些在本项目 src 里一个字都搜不到，于是**全部
 * 走外网**。这是个要在内网/离线跑的监控工具，那就是一片空白图标 —— 而且不报错。
 *
 * 这一路**不套 PREFIXES 白名单**（库里用了哪些集是库说了算，列不全也会漂），改用
 * 「前缀确实是 @iconify/json 里的一个图标集 **且** 名字在该集里真能解析出来」这条
 * 更硬的判据。要同时满足这两条才会被当成图标名，误收的概率可以忽略。
 *
 * 库升级后图标有增减也不用管：每次 start / build 都重扫一遍，自动跟上。
 */
const LIB_SCAN_DIRS = ["node_modules/@hsu-react/ui/es"];

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

const ICON_NAME_RE =
  /["'`]([a-z0-9]+(?:-[a-z0-9]+)*):([a-z0-9]+(?:-[a-z0-9]+)*)["'`]/g;

function scan(dir, onHit) {
  for (const file of walk(dir)) {
    const code = fs.readFileSync(file, "utf8");
    let m;
    ICON_NAME_RE.lastIndex = 0;
    while ((m = ICON_NAME_RE.exec(code))) onHit(m[1], m[2]);
  }
}

/**
 * 收集候选图标名。两路来源，判据不同：
 *
 * - `src`：按 PREFIXES 白名单收，**不验证名字是否存在** —— 存不存在留给后面报，
 *   这样把 `ph:` 图标名拼错了才会有一行「查无此图标」，而不是悄悄少一枚。
 * - `LIB_SCAN_DIRS`：库里用了哪些集不受我们控制，改用 `isRealIcon` 兜底（前缀是
 *   真图标集 + 名字真能解析），不认识的字面量直接丢掉、不报噪音。
 *
 * 两路都是纯文本匹配、不区分注释与代码：注释里写成 `` `ph:caret-down` `` 的举例也
 * 会被当真收进来。刻意接受 —— 想精确就得上 AST，而多收一枚不过几百字节。
 */
function collectCandidates(isRealIcon) {
  const byPrefix = new Map();
  const add = (prefix, name) => {
    if (!byPrefix.has(prefix)) byPrefix.set(prefix, new Set());
    byPrefix.get(prefix).add(name);
  };

  scan(SRC_DIR, (prefix, name) => {
    if (PREFIXES.includes(prefix)) add(prefix, name);
  });

  let fromLib = 0;
  for (const rel of LIB_SCAN_DIRS) {
    const dir = path.join(ROOT, rel);
    if (!fs.existsSync(dir)) {
      console.warn(`[gen:icons] 找不到待扫目录 ${rel}，跳过（依赖没装？）`);
      continue;
    }
    scan(dir, (prefix, name) => {
      if (!isRealIcon(prefix, name)) return;
      add(prefix, name);
      fromLib += 1;
    });
  }

  return { byPrefix, fromLib };
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

/** 按前缀懒加载整集并缓存 —— 整集动辄几 MB，用到哪套才读哪套 */
function makeSourceLoader() {
  const cache = new Map();

  return (prefix) => {
    if (cache.has(prefix)) return cache.get(prefix);

    const file = path.join(JSON_DIR, `${prefix}.json`);
    let source = null;
    if (fs.existsSync(file)) {
      try {
        source = JSON.parse(fs.readFileSync(file, "utf8"));
      } catch (err) {
        console.warn(`[gen:icons] ${prefix}.json 解析失败：${err.message}`);
      }
    }
    cache.set(prefix, source);
    return source;
  };
}

function main() {
  if (!fs.existsSync(JSON_DIR)) {
    console.error(
      `[gen:icons] 找不到 ${path.relative(ROOT, JSON_DIR)}，请先装依赖（@iconify/json 是 devDependency）`
    );
    process.exit(1);
  }

  const loadSource = makeSourceLoader();
  /** 扫库那一路的判据：前缀是真图标集，且名字在该集里真解析得出来 */
  const isRealIcon = (prefix, name) => {
    const source = loadSource(prefix);
    if (!source) return false;
    return resolveIcon(source, { icons: {} }, name);
  };

  const { byPrefix: candidates, fromLib } = collectCandidates(isRealIcon);

  const collections = [];
  const missing = [];
  let total = 0;

  // 扫依赖可能带来白名单之外的集合，所以取并集而不是只走 PREFIXES
  const prefixes = [...new Set([...PREFIXES, ...candidates.keys()])].sort();

  for (const prefix of prefixes) {
    const names = candidates.get(prefix);
    if (!names || names.size === 0) continue;

    const source = loadSource(prefix);
    if (!source) {
      console.warn(`[gen:icons] 找不到图标集 ${prefix}.json，跳过`);
      continue;
    }

    const target = { prefix, icons: {} };
    for (const key of ROOT_KEYS) {
      if (source[key] !== undefined) target[key] = source[key];
    }

    let hit = 0;
    for (const name of [...names].sort()) {
      const full = `${prefix}:${name}`;
      if (resolveIcon(source, target, name)) {
        hit += 1;
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
    `[gen:icons] 已生成 ${collections.length} 个精简图标集、${total} 枚图标，共 ${size} B（${(size / 1024).toFixed(1)} KB）` +
      `；其中扫依赖命中 ${fromLib} 处`
  );
  if (missing.length) {
    console.log(
      `[gen:icons] 以下字面量像图标名但集合里查无此图标，已忽略：\n  ${missing.join("\n  ")}`
    );
  }
}

main();
