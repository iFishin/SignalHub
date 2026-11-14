#!/bin/sh
# 重画 README 首图与 docs/assets/scenes/*.gif。和旧的 render-demo.sh 一样留个
# 薄壳，是为了 README / 文档里那句「跑 xxx 重画」不依赖调用者知道用哪个解释器。
#
#     npm run build && sh scripts/render-scenes.sh
#     sh scripts/render-scenes.sh --only day
#
# 真正的编排在 render-scenes.mjs（起本地服务、逐帧截图、ffmpeg 合成）。
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
node "$ROOT/scripts/render-scenes.mjs" "$@"
