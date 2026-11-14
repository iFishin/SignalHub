#!/bin/sh
# 【已停用 · 历史存档】旧版 README 首图的手绘生成器。
#
#     sh scripts/render-demo.sh [输出路径]
#
# 0.6.8 起 docs/assets/demo.gif 改由 scripts/render-scenes.sh 从产品组件逐帧渲染。
# 这个脚本画的是「照着 state.css 的常量抄一遍」的示意图，颜色/节奏会和代码漂移，
# 而且会覆盖新的首图 —— 所以默认拒绝运行。真要复现旧素材，显式加 --legacy。
#
# 两步：Swift（AppKit）逐帧画 PNG，ffmpeg 两遍合成 GIF。编排必须放在 shell 里：
# Swift 解释器在受限环境下 fork 子进程会被系统终止；而把 palettegen 和 paletteuse
# 写成一条滤镜链又会让 ffmpeg 把上百帧全缓进内存，同样会被杀。
set -eu

if [ "${1:-}" != "--legacy" ]; then
  echo "demo.gif 现在由 sh scripts/render-scenes.sh 生成（真实组件渲染）。" >&2
  echo "本脚本是已停用的手绘存档，运行会覆盖新的首图；确需使用请加 --legacy。" >&2
  exit 1
fi
shift

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-$ROOT/docs/assets/demo.gif}"
DIR="$(mktemp -d)"
trap 'rm -rf "$DIR"' EXIT

swift "$ROOT/scripts/render-demo.swift" "$DIR"

# 帧率只由 render-demo.swift 的 FPS 常量决定，这里读回来而不是再抄一遍数字。
FPS="$(cat "$DIR/fps")"
ffmpeg -nostdin -loglevel error -y -framerate "$FPS" -i "$DIR/f%04d.png" \
  -vf "palettegen=stats_mode=diff" -frames:v 1 "$DIR/palette.png"
ffmpeg -nostdin -loglevel error -y -framerate "$FPS" -i "$DIR/f%04d.png" \
  -i "$DIR/palette.png" \
  -filter_complex "[0:v][1:v]paletteuse=dither=bayer:bayer_scale=4" \
  -loop 0 "$OUT"

echo "wrote $OUT"
ls -lh "$OUT"
