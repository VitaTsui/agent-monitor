# 品牌图形

`icon.svg` 是应用图标的**唯一真源**，三端所有位图都由它生成。改图标只改这里，然后重跑下面的命令。

- `icon.svg` —— 完整版：`#18181b` 圆角方块底 + `#fafafa` 前景（终端提示符 `>` 与实心光标）
- `icon-mono.svg` —— 单色版：只有前景、`fill="currentColor"`，供应用内当内联图标用

配色跟随 shadcn/ui 默认主题（zinc-900 / zinc-50），与 `agent-monitor-web/src/styles/tokens.scss`
里的 `--primary` / `--primary-foreground` 同值。**不要加渐变**——shadcn 默认主题不用渐变。

## 重新生成三端位图

```bash
python3 assets/brand/build-icons.py
```

覆盖范围：

| 端 | 文件 |
| --- | --- |
| web | `agent-monitor-web/public/` 下 favicon.ico / favicon.svg / apple-touch-icon.png / pwa-192.png / pwa-512.png |
| 客户端（Tauri） | `agent-task-monitor/client/icons/` 下 32x32 / 128x128 / 128x128@2x / icon.png / icon.ico / icon.icns |
| iOS | `mobile-app/ios/.../AppIcon-512@2x.png`（1024×1024，**不含 alpha**，App Store 要求） |
| Android | 五档 mipmap 的 ic_launcher / _round / _foreground，以及 `drawable-v24/ic_launcher_foreground.xml` |

改 favicon 后记得把 `public/index.html` 里的 `?v=N` 加一，否则老访客的标签图标不会刷新。
