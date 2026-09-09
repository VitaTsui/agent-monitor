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

## macOS 的图标必须留白

`.icns` 里的图形只占画布 **82%**，四周透明——`build-icons.py` 里由 `with_macos_padding()`
处理，**只对 .icns 生效**。

82% 是量出来的，不是抄文档：Apple 文档写的是 80%（1024 画布里 824），按它做出来在程序坞里
偏小；量本机 Notes / Mail / Safari / Music 四个系统应用的 icns，不透明区占比**全部是 82.0%**。
以系统应用为准。

原因：macOS 的程序坞/访达不会替你缩放图标，系统自带应用的图形本身就带这圈留白；
满幅的图标放进去会比邻居明显大一圈。iOS / Android / web 相反——系统自己做圆角裁切
与缩放，满幅才对，加了留白反而显小。

这个坑踩过两次：仓库里曾有过一条 `session/mac-icon-padding` 分支，但只改了打包产物、
没回写到源文件，于是下一次从源重新生成时留白又没了。所以**别在打包产物上改图标**，
一律改 `icon.svg` 后重跑脚本。

改 favicon 后记得把 `public/index.html` 里的 `?v=N` 加一，否则老访客的标签图标不会刷新。
