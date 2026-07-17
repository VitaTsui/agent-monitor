/**
 * 危险输入防护：发布到终端会话前，检测类似 Claude Code bypass 权限、
 * 破坏性 shell 命令等高危内容，命中后由 UI 走多重确认。
 * 防护开关与自定义模式持久化到 localStorage（仅本机生效）。
 */

const STORAGE_KEY = "AM_DANGER_GUARD";

/** 内置危险模式（正则、不区分大小写；`pattern` 展示用，`test` 实际匹配） */
export const BUILTIN_DANGER_PATTERNS: {
  pattern: string;
  test: RegExp;
  desc: string;
}[] = [
  {
    pattern: "--dangerously-skip-permissions",
    test: /--dangerously-skip-permissions/i,
    desc: "Claude Code 跳过权限确认（bypass permissions）",
  },
  {
    pattern: "bypass permissions",
    test: /bypass\s+permissions/i,
    desc: "Claude Code 权限绕过模式",
  },
  {
    pattern: "/permissions",
    // 仅匹配作为斜杠命令出现（行首或空白后），避免误伤路径文本
    test: /(^|\s)\/permissions\b/i,
    desc: "修改 Claude Code 权限配置",
  },
  { pattern: "rm -rf", test: /\brm\s+-[a-z]*rf?\b|\brm\s+-fr\b/i, desc: "递归强制删除文件" },
  { pattern: "sudo", test: /\bsudo\s/i, desc: "以管理员权限执行命令" },
  { pattern: "chmod 777", test: /\bchmod\s+-?[a-z]*\s*777\b/i, desc: "开放全部文件权限" },
  {
    pattern: "git push --force",
    test: /\bgit\s+push\b[^\n]*(--force\b|-f\b)/i,
    desc: "强制推送覆盖远端历史",
  },
  { pattern: "--no-verify", test: /--no-verify\b/i, desc: "跳过校验钩子" },
  {
    pattern: "| sh / | bash",
    test: /\|\s*(sh|bash|zsh)\b/i,
    desc: "管道执行脚本",
  },
];

export interface DangerGuardConfig {
  /** 防护总开关（默认开） */
  enabled: boolean;
  /** 用户自定义模式（子串匹配，一行一个） */
  customPatterns: string[];
}

export function loadGuardConfig(): DangerGuardConfig {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) {
      const parsed = JSON.parse(raw) as Partial<DangerGuardConfig>;
      return {
        enabled: parsed.enabled !== false,
        customPatterns: Array.isArray(parsed.customPatterns)
          ? parsed.customPatterns.filter((s) => typeof s === "string" && s.trim())
          : [],
      };
    }
  } catch {
    // 解析失败按默认处理
  }
  return { enabled: true, customPatterns: [] };
}

export function saveGuardConfig(config: DangerGuardConfig) {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(config));
}

export interface DangerHit {
  pattern: string;
  desc: string;
}

/** 检测文本命中的危险模式；未开启防护或未命中返回空数组 */
export function checkDanger(text: string): DangerHit[] {
  const config = loadGuardConfig();
  if (!config.enabled) {
    return [];
  }

  const lower = text.toLowerCase();
  const hits: DangerHit[] = [];

  BUILTIN_DANGER_PATTERNS.forEach((p) => {
    if (p.test.test(text)) {
      hits.push({ pattern: p.pattern, desc: p.desc });
    }
  });
  config.customPatterns.forEach((pattern) => {
    if (pattern.trim() && lower.includes(pattern.trim().toLowerCase())) {
      hits.push({ pattern: pattern.trim(), desc: "自定义危险模式" });
    }
  });

  return hits;
}

/** 二次确认需要输入的确认词 */
export const CONFIRM_WORD = "允许执行";
