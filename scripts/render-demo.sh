#!/bin/sh
# 重画 README 顶部的 docs/assets/demo.gif。
#
#     sh scripts/render-demo.sh [输出路径]
#
# 两步：Swift（AppKit）逐帧画 PNG，ffmpeg 两遍合成 GIF。编排必须放在 shell 里：
# Swift 解释器在受限环境下 fork 子进程会被系统终止；而把 palettegen 和 paletteuse
# 写成一条滤镜链又会让 ffmpeg 把上百帧全缓进内存，同样会被杀。
set -eu

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
