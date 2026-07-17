import React, { useEffect, useMemo } from "react";
import { ConfigProvider, theme as antdTheme } from "antd";
import { observer } from "mobx-react-lite";
import classNames from "classnames";
import styles from "./index.module.scss";
import ThemeStore from "./ThemeStore";

interface ThemeProps {
  children?: React.ReactNode;
}

// CF 调色板：以 CSS 变量下发，亮/暗两套，内容区样式统一引用
const PALETTE = {
  light: {
    canvas: "#f6f7f9", // 页面画布
    surface: "#ffffff", // 卡片/面板底
    subtle: "#fafbfc", // 表头/次级底
    border: "#e6e8eb", // 描边
    borderWeak: "#eceef1", // 更轻描边
    headerBg: "#ffffff", // 顶栏底
    text: "rgba(0, 0, 0, 0.88)",
    text2: "rgba(0, 0, 0, 0.45)",
    text3: "rgba(0, 0, 0, 0.35)",
    rowHover: "#f6f7f9",
  },
  dark: {
    canvas: "#131314",
    surface: "#1c1c1e",
    subtle: "#232325",
    border: "rgba(255, 255, 255, 0.12)",
    borderWeak: "rgba(255, 255, 255, 0.08)",
    headerBg: "#1c1c1e",
    text: "rgba(255, 255, 255, 0.85)",
    text2: "rgba(255, 255, 255, 0.45)",
    text3: "rgba(255, 255, 255, 0.35)",
    rowHover: "rgba(255, 255, 255, 0.04)",
  },
};

const Theme: React.FC<ThemeProps> = observer((props) => {
  const { children } = props;
  const { isDark, primaryColor } = ThemeStore;

  const p = isDark ? PALETTE.dark : PALETTE.light;

  // 暗色下把水青调暗去刺眼（亮色保持品牌色）
  const accent = isDark ? "#1C9C9C" : primaryColor;

  // 主色 hex -> rgba，用于菜单高亮
  const hexToRgba = (hex: string, alpha: number) => {
    const m = hex.replace("#", "");
    if (m.length < 6) return hex;
    const r = parseInt(m.substring(0, 2), 16);
    const g = parseInt(m.substring(2, 4), 16);
    const b = parseInt(m.substring(4, 6), 16);
    return `rgba(${r}, ${g}, ${b}, ${alpha})`;
  };

  // 暗色导航：中性黑（渐变两端 + 中间色作为 token 兜底）
  const DARK_TOP = "#1F1F1F";
  const DARK_BOTTOM = "#141414";

  const headerBg = p.headerBg;
  const siderBg = p.surface;

  // CF 调色板挂到 <html>，保证 body 级 Portal（Modal/Drawer/下拉）也能解析这些变量
  useEffect(() => {
    const root = document.documentElement;
    const vars: Record<string, string> = {
      "--primary-color": accent,
      "--cf-canvas": p.canvas,
      "--cf-surface": p.surface,
      "--cf-subtle": p.subtle,
      "--cf-border": p.border,
      "--cf-border-weak": p.borderWeak,
      "--cf-text": p.text,
      "--cf-text-2": p.text2,
      "--cf-text-3": p.text3,
    };
    Object.entries(vars).forEach(([k, v]) => root.style.setProperty(k, v));
  }, [accent, p]);

  const themeConfig = useMemo(
    () => ({
      algorithm: isDark
        ? antdTheme.darkAlgorithm
        : antdTheme.defaultAlgorithm,
      token: {
        colorPrimary: accent,
        // Cloudflare 观感：以描边替代阴影、统一中等圆角
        borderRadius: 8,
        borderRadiusLG: 8,
        borderRadiusSM: 6,
        // 冷调浅灰画布 + 轻描边（随外观切换）
        colorBgLayout: p.canvas,
        colorBorder: p.border,
        colorBorderSecondary: p.borderWeak,
      },
      components: {
        Layout: {
          headerBg,
          siderBg,
        },
        // CF 风格：方正紧凑的控件
        Button: {
          controlHeight: 34,
          borderRadius: 6,
          primaryShadow: "none",
          defaultShadow: "none",
          fontWeight: 500,
        },
        Input: { borderRadius: 6 },
        Select: { borderRadius: 6 },
        // CF 风格：浅头、细分割线、行 hover
        Table: {
          headerBg: p.subtle,
          headerColor: p.text2,
          headerSplitColor: "transparent",
          borderColor: p.borderWeak,
          rowHoverBg: p.rowHover,
          cellPaddingBlock: 12,
          headerBorderRadius: 8,
        },
        Card: {
          borderRadiusLG: 8,
        },
        Menu: {
          // 透明底以透出导航渐变；选中态为圆角胶囊
          darkItemBg: "transparent",
          darkSubMenuItemBg: "transparent",
          darkPopupBg: p.surface,
          darkItemColor: "rgba(255, 255, 255, 0.82)",
          darkItemHoverColor: "#ffffff",
          darkItemHoverBg: "rgba(255, 255, 255, 0.14)",
          darkItemSelectedBg: hexToRgba(accent, 0.16),
          darkItemSelectedColor: accent,
          // 亮色模式（CF 侧边栏）：紧凑行 + 浅水色高亮，左侧高亮条由 CSS 叠加
          itemSelectedBg: hexToRgba(accent, 0.12),
          itemSelectedColor: accent,
          itemActiveBg: hexToRgba(accent, 0.12),
          itemHoverBg: isDark
            ? "rgba(255, 255, 255, 0.06)"
            : "rgba(0, 0, 0, 0.04)",
          itemColor: p.text,
          itemBorderRadius: 6,
          itemMarginInline: 8,
          itemMarginBlock: 2,
          itemHeight: 38,
          horizontalItemHoverBg: "#0000000a",
        },
      },
    }),
    [isDark, accent, p, headerBg, siderBg]
  );

  return (
    <ConfigProvider wave={{ disabled: true }} theme={themeConfig}>
      <div
        className={classNames(styles.theme, isDark ? "dark" : "light")}
        style={
          {
            "--primary-color": accent,
            "--nav-dark-top": DARK_TOP,
            "--nav-dark-bottom": DARK_BOTTOM,
            // CF 调色板（内容区样式统一引用）
            "--cf-canvas": p.canvas,
            "--cf-surface": p.surface,
            "--cf-subtle": p.subtle,
            "--cf-border": p.border,
            "--cf-border-weak": p.borderWeak,
            "--cf-text": p.text,
            "--cf-text-2": p.text2,
            "--cf-text-3": p.text3,
          } as React.CSSProperties
        }
      >
        {children}
      </div>
    </ConfigProvider>
  );
});

export default Theme;
