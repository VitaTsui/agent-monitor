#!/usr/bin/env python3
"""从 assets/brand/icon.svg 生成三端全部图标位图。

图标只有一份真源（icon.svg），三端的十几个文件都是它的派生物。手工逐个改必然漏——
这次替换就涉及 35 个文件、横跨 web / Tauri / iOS / Android 四套规格。

依赖只有 pillow。**刻意不用 cairosvg** —— 它要装原生 cairo 库，在干净的 mac 上装不上，
而这个图形只有「圆角方块 + 一条折线 + 一个方块」三个图元，用 Pillow 直接画即可，
几何与 icon.svg 逐点对齐（实测差异 0.11%，只在抗锯齿边缘）。

代价是改了 icon.svg 要同步改下面的 draw()。图形本身很少动，这个代价比让每个人先装
cairo 划算；真要换成复杂图形时再引渲染器不迟。

macOS 上 .icns 用系统自带的 iconutil 打包，比 Pillow 直接写更规范。

用法：python3 assets/brand/build-icons.py
"""
import math
import os
import shutil
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
SRC = os.path.join(ROOT, "assets/brand/icon.svg")

try:
    from PIL import Image, ImageDraw
except ImportError:
    sys.exit("缺依赖：pip install pillow")

BG = "#18181b"   # 与 tokens.scss 的 --primary 同值
FG = "#fafafa"   # --primary-foreground


def render(size: int) -> "Image.Image":
    """按 icon.svg 的几何画出指定边长的 RGBA 图。

    超采样 8 倍再缩回来：Pillow 的 line/rectangle 没有抗锯齿，直接按目标尺寸画
    在 16px 这种小图上会出锯齿。
    """
    ss = 8
    w = size * ss
    im = Image.new("RGBA", (w, w), (0, 0, 0, 0))
    d = ImageDraw.Draw(im)
    k = w / 64.0  # icon.svg 的 viewBox 是 0 0 64 64

    d.rounded_rectangle([0, 0, w - 1, w - 1], radius=14 * k, fill=BG)

    # `>`：对应 SVG 里 stroke-width 7 / linecap square / linejoin miter 的折线
    sw = 7 * k
    pts = [(16 * k, 20 * k), (30 * k, 32 * k), (16 * k, 44 * k)]
    d.line(pts, fill=FG, width=int(round(sw)), joint="curve")
    # square cap：Pillow 只有平头，两端各沿反方向补半个线宽
    for (x, y), (nx, ny) in [(pts[0], pts[1]), (pts[2], pts[1])]:
        ang = math.atan2(y - ny, x - nx)
        d.line([(x, y), (x + math.cos(ang) * sw / 2, y + math.sin(ang) * sw / 2)],
               fill=FG, width=int(round(sw)))

    # 光标方块
    d.rectangle([44 * k, 39.5 * k, 53 * k, 48.5 * k], fill=FG)

    return im.resize((size, size), Image.LANCZOS)


def with_macos_padding(img: "Image.Image") -> "Image.Image":
    """按 Apple 规范给 macOS 图标加留白：图形占画布 82%，四周透明。

    macOS 的程序坞/访达**不会**替你缩放图标——系统自带应用的图形本身就带这圈留白，
    满幅的图标放进去会比邻居明显大一圈。iOS / Android / web 相反：系统自己做圆角裁切
    与缩放，满幅才对。所以只有 .icns 走这条。

    82% 不是拍的：量了本机 Notes / Mail / Safari / Music 四个系统应用的 icns，
    不透明区占比全部是 82.0%。先按 Apple 文档写的 80%（1024 里 824）做，实际偏小一点。
    """
    w = img.width
    inner = round(w * 0.82)
    canvas = Image.new("RGBA", (w, w), (0, 0, 0, 0))
    small = img.resize((inner, inner), Image.LANCZOS)
    off = (w - inner) // 2
    canvas.paste(small, (off, off), small)
    return canvas


def save(img: "Image.Image", path: str) -> None:
    full = os.path.join(ROOT, path)
    os.makedirs(os.path.dirname(full), exist_ok=True)
    img.save(full)
    print(f"  {path}  {img.width}x{img.height}")


base = render(1024)
print("web")
save(render(180), "agent-monitor-web/public/apple-touch-icon.png")
save(render(192), "agent-monitor-web/public/pwa-192.png")
save(render(512), "agent-monitor-web/public/pwa-512.png")
shutil.copy(SRC, os.path.join(ROOT, "agent-monitor-web/public/favicon.svg"))
print("  agent-monitor-web/public/favicon.svg")
ico = os.path.join(ROOT, "agent-monitor-web/public/favicon.ico")
base.save(ico, format="ICO", sizes=[(16, 16), (32, 32), (48, 48)])
print("  agent-monitor-web/public/favicon.ico  16/32/48")

print("客户端（Tauri）")
for name, size in [("32x32.png", 32), ("128x128.png", 128), ("128x128@2x.png", 256), ("icon.png", 512)]:
    save(render(size), f"agent-task-monitor/client/icons/{name}")
tauri_ico = os.path.join(ROOT, "agent-task-monitor/client/icons/icon.ico")
base.save(tauri_ico, format="ICO", sizes=[(16, 16), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
print("  agent-task-monitor/client/icons/icon.ico  16..256")

# .icns：iconutil 只认 .iconset 目录里的固定命名
with tempfile.TemporaryDirectory() as tmp:
    iconset = os.path.join(tmp, "am.iconset")
    os.makedirs(iconset)
    for size, name in [(16, "16x16"), (32, "16x16@2x"), (32, "32x32"), (64, "32x32@2x"),
                       (128, "128x128"), (256, "128x128@2x"), (256, "256x256"),
                       (512, "256x256@2x"), (512, "512x512"), (1024, "512x512@2x")]:
        with_macos_padding(render(size)).save(os.path.join(iconset, f"icon_{name}.png"))
    icns = os.path.join(ROOT, "agent-task-monitor/client/icons/icon.icns")
    subprocess.run(["iconutil", "-c", "icns", iconset, "-o", icns], check=True)
    print("  agent-task-monitor/client/icons/icon.icns")

print("iOS")
# App Store 拒收带 alpha 的图标，先压到不透明底上
ios = Image.new("RGB", base.size, "#18181b")
ios.paste(base, mask=base.split()[3])
save(ios, "mobile-app/ios/App/App/Assets.xcassets/AppIcon.appiconset/AppIcon-512@2x.png")

print("Android")
for d, s in {"mdpi": 48, "hdpi": 72, "xhdpi": 96, "xxhdpi": 144, "xxxhdpi": 192}.items():
    p = f"mobile-app/android/app/src/main/res/mipmap-{d}"
    img = render(s)
    save(img, f"{p}/ic_launcher.png")

    mask = Image.new("L", (s, s), 0)
    ImageDraw.Draw(mask).ellipse((0, 0, s - 1, s - 1), fill=255)
    rnd = img.copy()
    rnd.putalpha(mask)
    save(rnd, f"{p}/ic_launcher_round.png")

    # 自适应图标的前景层：各家启动器会按自己的形状裁切，主体必须收在中心安全区内
    fg = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    inner = render(int(s * 0.66))
    fg.paste(inner, ((s - inner.width) // 2, (s - inner.height) // 2), inner)
    save(fg, f"{p}/ic_launcher_foreground.png")

print("\n完成。改了 favicon 记得把 public/index.html 里的 ?v=N 加一，否则老访客不刷新。")
