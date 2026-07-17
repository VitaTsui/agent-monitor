import wsCache, { CACHE_KEY } from "@/utils/wsCache";
import { makeAutoObservable } from "mobx";

export type Layout = "left" | "top" | "mixed";

export type Theme = "light" | "dark";

export type HeaderTheme = "light" | "dark" | "theme";

// CF 做法：外观三档（亮 / 暗 / 跟随系统）
export type Appearance = "light" | "dark" | "system";

class ThemeStore {
  // 用户选择的外观档位
  get appearance() {
    return this._appearance;
  }
  private _appearance: Appearance = "system";

  // 系统当前是否暗色（外观=system 时生效）
  private _systemDark = false;

  // 解析后的实际暗色状态
  get isDark(): boolean {
    if (this._appearance === "system") return this._systemDark;
    return this._appearance === "dark";
  }

  // CF 风格：固定左侧布局，不再提供切换
  get layout(): Layout {
    return "left";
  }

  // 导航明暗跟随外观
  get headerTheme(): HeaderTheme {
    return this.isDark ? "dark" : "light";
  }

  // 内容主题跟随外观
  get theme(): Theme {
    return this.isDark ? "dark" : "light";
  }

  // 固定水青主色
  get primaryColor() {
    return "#13C2C2";
  }

  constructor() {
    makeAutoObservable(this);

    const cached = wsCache.get(CACHE_KEY.APPEARANCE) as Appearance | undefined;
    if (cached === "light" || cached === "dark" || cached === "system") {
      this._appearance = cached;
    }

    // 监听系统配色，外观=system 时实时跟随
    if (typeof window !== "undefined" && window.matchMedia) {
      const mq = window.matchMedia("(prefers-color-scheme: dark)");
      this._systemDark = mq.matches;
      mq.addEventListener("change", (e) => this._setSystemDark(e.matches));
    }
  }

  private _setSystemDark = (v: boolean) => {
    this._systemDark = v;
  };

  public setAppearance = (appearance: Appearance) => {
    this._appearance = appearance;
    wsCache.set(CACHE_KEY.APPEARANCE, appearance);
  };
}

export default new ThemeStore();
