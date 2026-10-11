# 图标源文件

- `app-icon.png` —— 应用图标的源图（1254×1254，带透明）。
  改完用它重新生成全套尺寸：`npx tauri icon design/app-icon.png`
- `app-icon-raw.png` —— 最初拿到的版本。它四周的「透明」其实是**画进图里的
  棋盘格**（浅灰方格、alpha 全是不透明的 255），直接用会在图标周围带一圈棋盘格。
  `app-icon.png` 是把背景抠掉后的结果，两个都留着，以便回溯抠得对不对。

托盘图标不在这里 —— 它是三个圆的几何图形，由 `src-tauri/src/tray.rs` 里的
`tray_glyph_rgba()` 直接画出来（只是 alpha、没有颜色，交给系统按菜单栏明暗反色）。
