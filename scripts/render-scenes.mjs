#!/usr/bin/env node
/**
 * 重画 README 顶部的 docs/assets/demo.gif 与 docs/assets/scenes/*.gif。
 *
 *     npm run build && node scripts/render-scenes.mjs
 *     node scripts/render-scenes.mjs --only day          # 只重画一段
 *     node scripts/render-scenes.mjs --keep-frames       # 保留临时 PNG 便于核对
 *
 * **这不是手绘，也不是真窗口录屏**：从 `dist/`（刚构建出来的产品代码）起一个
 * 本地静态服务，用 Chrome 调试通道逐帧截 `?window=demo` 的画面，再由 ffmpeg
 * 合成 GIF。演示页渲染的是产品自己的 `SignalLamp` / `InboxWindow` / `LampMenu`
 * 组件和 `lamp.css` / `inbox.css` / `state.css`，喂进去的是 `demo.ts` 的剧本数据；
 * 所以灯珠颜色、闪烁周期、圆角方灯珠、收件箱分段、圆环四项都跟着代码走。
 * 录图只做两件事：把演示页的标题 / 场景条 / 图注 / 播放控件移出画面，以及用
 * 深链 `&demo=<场景>&step=N` 停到指定帧。舞台那块深色底是演示页的画框色
 * （`#1b1b20`），不是壁纸 —— 这一点在 docs/scenes.md 里如实写明。
 *
 * 为什么逐帧截而不是录一段视频：每一帧都对应演示页里一个可复现的 step，
 * 场景之间用「点产品自己的进度条」切换，不重新加载页面，于是 CSS 呼吸动画
 * 从头到尾是连续的、状态跃迁也是产品自己的行为。`--virtual-time-budget` 那种
 * 一次性截图推进不了 CSS 动画相位，会得到一张不会闪的幻灯片。
 *
 * 依赖：本机 Chrome（可用 CHROME 环境变量指定）+ PATH 上的 ffmpeg + Node 22
 * （全局 WebSocket）。没有第三方 npm 包。
 */
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const DIST = path.join(ROOT, "dist");
const OUT_DIR = path.join(ROOT, "docs", "assets");
const SCENE_DIR = path.join(OUT_DIR, "scenes");
const DEMO_OUT = path.join(OUT_DIR, "demo.gif");

const W = 720;
const H = 420;
const FPS = 8; // 场景动图
const DEMO_FPS = 10; // README 首图（灯效节奏更细）
const HOLD_MS = 1100; // 每个 step 停多久（含该状态下真实的呼吸相位）
const STABLE_MS = 320; // 导航后等 React 挂载 + 字体落定
const STEP_SETTLE_MS = 160; // 点进度条后等 React 重渲染

/** 与 src/lib/demo.ts 的 DEMO_SCENARIOS 一一对应；steps 由 assertSceneSteps 校对。 */
const SCENES = [
  { id: "day", steps: 7, title: "一天的节奏" },
  { id: "asking", steps: 3, title: "有事要你" },
  { id: "silence", steps: 3, title: "它不说话了" },
  { id: "tucked", steps: 3, title: "收起来的样子" },
  { id: "look", steps: 9, title: "用光说话" },
  { id: "inbox", steps: 6, title: "收件箱与审批" },
  { id: "callme", steps: 5, title: "Call Me 留言" },
  { id: "menu", steps: 4, title: "灯上的手势" },
];

/**
 * README 首图的镜头表：讲「灯效的节奏」，所以跨场景挑状态，而不是复述某一段
 * 剧情。顺序沿用旧 demo.gif 的叙事（空闲 → 快闪 → 慢闪 → 授权 → 关注 → 出错
 * → 完成 → 沉默 → 无会话 → 安静模式），但每一帧都换成产品真实组件。
 */
const DEMO_REEL = [
  { scene: "day", step: 1, ms: 900 }, // idle · 绿常亮
  { scene: "day", step: 2, ms: 1400 }, // thinking · 绿快闪
  { scene: "day", step: 3, ms: 1400 }, // working · 绿慢闪 + 多组错相
  { scene: "day", step: 4, ms: 1400 }, // permission · 红平滑闪 + 圆角方灯珠
  { scene: "asking", step: 1, ms: 1000 }, // attention · 黄闪
  { scene: "asking", step: 3, ms: 1000 }, // error · 红频闪
  { scene: "day", step: 6, ms: 900 }, // done · 常亮 + 跃迁脉冲
  { scene: "silence", step: 1, ms: 900 }, // stale · 灰常亮
  { scene: "silence", step: 3, ms: 800 }, // offline · 更淡
  { scene: "tucked", step: 1, ms: 900 }, // quiet · 半透明牌
];

/** 录图专用样式：只把演示页外壳移走，绝不改产品组件自己的 CSS。 */
const CAPTURE_CSS = `
/* 由 scripts/render-scenes.mjs 注入，只在录图副本里存在。 */
html, body { margin: 0; padding: 0; overflow: hidden; background: #1b1b20; }
.demo-window { padding: 0; gap: 0; min-height: 0; background: #1b1b20; }
.demo-window-head, .demo-scenarios, .demo-caption, .demo-badge { display: none !important; }
/* 播放控件不能 display:none —— 生成器要靠点 .demo-progress-seg 切帧。
   挪出视口而不是删掉，切帧走的就仍然是产品自己的 goTo()。 */
.demo-controls { position: fixed; left: -9999px; top: 0; width: 1px; height: 1px; overflow: hidden; }
.demo { width: 100%; max-width: none; margin: 0; padding: 0; gap: 0; }
.demo-stage,
.demo-stage[data-stage="panel"],
.demo[data-compact] .demo-stage,
.demo[data-compact] .demo-stage[data-stage="panel"] {
  width: 100%; height: 100vh; border: 0; border-radius: 0;
  background: #1b1b20; padding: 20px 12px 12px !important;
}
`;

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".png": "image/png",
  ".svg": "image/svg+xml",
  ".json": "application/json",
};

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function findChrome() {
  if (process.env.CHROME) return process.env.CHROME;
  const candidates = [
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
  ];
  return candidates.find((p) => fs.existsSync(p));
}

/** 镜头表里的步数不能和 demo.ts 漂移：改了演示场景要同步这里。 */
function assertSceneSteps() {
  const src = fs.readFileSync(path.join(ROOT, "src", "lib", "demo.ts"), "utf8");
  const ids = SCENES.map((s) => s.id);
  for (let i = 0; i < ids.length; i++) {
    const id = ids[i];
    const hit = new RegExp(`^    id: "${id}",`, "m").exec(src);
    if (!hit) throw new Error(`demo.ts 里找不到场景 ${id}`);
    const rest = src.slice(hit.index + hit[0].length);
    const nextId = ids[i + 1];
    const boundary = nextId
      ? new RegExp(`^    id: "${nextId}",`, "m").exec(rest)
      : /^\];/m.exec(rest);
    const block = boundary ? rest.slice(0, boundary.index) : rest;
    const found = (block.match(/^        id: "[a-zA-Z]+",/gm) || []).length;
    const wanted = SCENES[i].steps;
    if (found !== wanted) {
      throw new Error(
        `场景 ${id} 的步数变了：demo.ts 有 ${found} 步，镜头表写着 ${wanted} 步。请同步 scripts/render-scenes.mjs 的 SCENES。`,
      );
    }
  }
}

function prepareBuild(work) {
  if (!fs.existsSync(path.join(DIST, "index.html"))) {
    throw new Error("找不到 dist/index.html —— 先跑 npm run build");
  }
  const site = path.join(work, "site");
  fs.cpSync(DIST, site, { recursive: true });
  const index = path.join(site, "index.html");
  let html = fs.readFileSync(index, "utf8");
  // 语言和深浅色必须在第一帧就定死，否则换一台机器录出来的图会变语言/变底。
  const inject = `<style>${CAPTURE_CSS}</style>
<script>
try { Object.defineProperty(navigator, "language", { get: () => "zh-CN", configurable: true }); } catch (_) {}
try { localStorage.setItem("signalhub-theme-resolved", "dark"); } catch (_) {}
document.documentElement.dataset.theme = "dark";
</script>`;
  if (!html.includes("</head>")) throw new Error("dist/index.html 结构变了，找不到 </head>");
  fs.writeFileSync(index, html.replace("</head>", `${inject}\n</head>`));
  return site;
}

function serve(site) {
  const server = http.createServer((req, res) => {
    const pathname = decodeURIComponent(new URL(req.url, "http://127.0.0.1").pathname);
    const file = path.resolve(site, `.${pathname === "/" ? "/index.html" : pathname}`);
    if (!file.startsWith(site)) {
      res.writeHead(403).end();
      return;
    }
    fs.readFile(file, (err, data) => {
      if (err) {
        res.writeHead(404).end();
        return;
      }
      res.writeHead(200, {
        "Content-Type": MIME[path.extname(file)] || "application/octet-stream",
        "Cache-Control": "no-store",
      });
      res.end(data);
    });
  });
  return new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server)));
}

function getJson(port, urlPath) {
  return new Promise((resolve, reject) => {
    http
      .get({ host: "127.0.0.1", port, path: urlPath }, (r) => {
        let data = "";
        r.on("data", (chunk) => (data += chunk));
        r.on("end", () => {
          try {
            resolve(JSON.parse(data));
          } catch (e) {
            reject(e);
          }
        });
      })
      .on("error", reject);
  });
}

/** 最小 CDP 客户端：只用到 navigate / evaluate / captureScreenshot。 */
async function connectCdp(wsUrl) {
  const ws = new WebSocket(wsUrl);
  await new Promise((resolve, reject) => {
    ws.onopen = resolve;
    ws.onerror = reject;
  });
  let seq = 0;
  const pending = new Map();
  ws.onmessage = (event) => {
    const msg = JSON.parse(event.data);
    if (msg.id && pending.has(msg.id)) {
      pending.get(msg.id)(msg);
      pending.delete(msg.id);
    }
  };
  const send = (method, params = {}) =>
    new Promise((resolve) => {
      const id = ++seq;
      pending.set(id, resolve);
      ws.send(JSON.stringify({ id, method, params }));
    });
  return { ws, send };
}

function ffmpeg(args) {
  const r = spawnSync("ffmpeg", ["-nostdin", "-loglevel", "error", "-y", ...args], {
    encoding: "utf8",
  });
  if (r.status !== 0) throw new Error(`ffmpeg 失败: ${r.stderr || r.status}`);
}

function makeGif(framesDir, out, fps) {
  const palette = path.join(framesDir, "palette.png");
  const input = ["-framerate", String(fps), "-i", path.join(framesDir, "f%04d.png")];
  ffmpeg([...input, "-vf", "palettegen=stats_mode=diff", "-frames:v", "1", palette]);
  ffmpeg([
    ...input,
    "-i",
    palette,
    "-filter_complex",
    "[0:v][1:v]paletteuse=dither=bayer:bayer_scale=4",
    "-loop",
    "0",
    out,
  ]);
  return fs.statSync(out).size;
}

async function main() {
  const args = process.argv.slice(2);
  const onlyIdx = args.indexOf("--only");
  const only = onlyIdx >= 0 ? args[onlyIdx + 1]?.split(",") : null;
  const keepFrames = args.includes("--keep-frames");
  if (onlyIdx >= 0 && !args[onlyIdx + 1]) throw new Error("--only 后面要给场景 id，逗号分隔");

  assertSceneSteps();
  const chromeBin = findChrome();
  if (!chromeBin) throw new Error("找不到 Chrome，用 CHROME=/path/to/chrome 指定");
  if (spawnSync("ffmpeg", ["-version"]).status !== 0) throw new Error("PATH 上找不到 ffmpeg");
  fs.mkdirSync(SCENE_DIR, { recursive: true });

  const work = fs.mkdtempSync(path.join(os.tmpdir(), "sh-scenes-"));
  const site = prepareBuild(work);
  const server = await serve(site);
  const sitePort = server.address().port;
  const chromeProfile = path.join(work, "chrome");
  fs.mkdirSync(chromeProfile, { recursive: true });
  const chrome = spawn(
    chromeBin,
    [
      "--headless=new",
      "--no-sandbox",
      "--disable-gpu",
      "--no-proxy-server",
      "--hide-scrollbars",
      "--disable-extensions",
      "--no-first-run",
      "--no-default-browser-check",
      "--remote-debugging-port=0",
      `--user-data-dir=${chromeProfile}`,
      `--window-size=${W},${H}`,
      "about:blank",
    ],
    { stdio: "ignore" },
  );

  let cdp;
  let navSeq = 0;
  try {
    const portFile = path.join(chromeProfile, "DevToolsActivePort");
    let port = 0;
    for (let i = 0; i < 100; i++) {
      if (fs.existsSync(portFile)) {
        port = Number(fs.readFileSync(portFile, "utf8").split("\n")[0]);
        if (port > 0) break;
      }
      await sleep(100);
    }
    if (!port) throw new Error("Chrome 没打开调试端口");
    const targets = await getJson(port, "/json/list");
    const page = targets.find((t) => t.type === "page");
    if (!page) throw new Error("Chrome 里没有可用页面");
    cdp = await connectCdp(page.webSocketDebuggerUrl);
    const { send } = cdp;
    await send("Page.enable");
    await send("Runtime.enable");
    await send("Emulation.setDeviceMetricsOverride", {
      width: W,
      height: H,
      deviceScaleFactor: 1,
      mobile: false,
    });

    const goto = async (scene, step) => {
      // r= 让 CDP 每次都真的重载；同时拿它当「新文档已经落地」的判据 ——
      // 只查 .demo-stage 的话，导航还没完成、旧页面仍在时就误判成好了。
      const marker = ++navSeq;
      const url = `http://127.0.0.1:${sitePort}/?window=demo&demo=${scene}&step=${step}&r=${marker}`;
      await send("Page.navigate", { url });
      let ready = false;
      for (let i = 0; i < 60; i++) {
        const r = await send("Runtime.evaluate", {
          expression: `location.search.includes("r=${marker}") && !!document.querySelector('.demo-progress-seg') && !!document.querySelector('.demo-stage')?.firstElementChild`,
          returnByValue: true,
        });
        if (r.result?.result?.value) {
          ready = true;
          break;
        }
        await sleep(80);
      }
      if (!ready) throw new Error(`${scene}/${step} 的舞台没挂载出来`);
      await sleep(STABLE_MS);
    };

    const clickStep = async (index) => {
      const r = await send("Runtime.evaluate", {
        expression: `(() => { const b = document.querySelectorAll('.demo-progress-seg'); if (!b[${index}]) return false; b[${index}].click(); return true; })()`,
        returnByValue: true,
      });
      if (!r.result?.result?.value) throw new Error(`第 ${index + 1} 个进度条按钮不存在`);
      await sleep(STEP_SETTLE_MS);
    };

    const record = async (ms, framesDir, fps, counter) => {
      const count = Math.max(1, Math.round((ms / 1000) * fps));
      const interval = 1000 / fps;
      for (let i = 0; i < count; i++) {
        const shot = await send("Page.captureScreenshot", {
          format: "png",
          captureBeyondViewport: false,
        });
        if (!shot.result?.data) throw new Error("截图失败");
        const name = `f${String(counter.value++).padStart(4, "0")}.png`;
        fs.writeFileSync(path.join(framesDir, name), Buffer.from(shot.result.data, "base64"));
        await sleep(interval);
      }
    };

    const targetsToRender = [
      ...SCENES.map((s) => ({ ...s, out: path.join(SCENE_DIR, `${s.id}.gif`), fps: FPS })),
      { id: "demo", out: DEMO_OUT, fps: DEMO_FPS },
    ].filter((t) => !only || only.includes(t.id));
    if (targetsToRender.length === 0) throw new Error(`--only 没匹配到任何输出：${only?.join(",")}`);

    for (const target of targetsToRender) {
      const framesDir = path.join(work, "frames", target.id);
      fs.mkdirSync(framesDir, { recursive: true });
      const counter = { value: 1 };
      if (target.id === "demo") {
        for (const shot of DEMO_REEL) {
          await goto(shot.scene, shot.step);
          await record(shot.ms, framesDir, target.fps, counter);
        }
      } else {
        await goto(target.id, 1);
        for (let i = 0; i < target.steps; i++) {
          if (i > 0) await clickStep(i);
          await record(HOLD_MS, framesDir, target.fps, counter);
        }
      }
      const bytes = makeGif(framesDir, target.out, target.fps);
      const seconds = ((counter.value - 1) / target.fps).toFixed(1);
      console.log(
        `✓ ${String(target.id).padEnd(8)} ${String(counter.value - 1).padStart(3)} 帧 / ${seconds}s / ${(bytes / 1024).toFixed(0)} KB → ${path.relative(ROOT, target.out)}`,
      );
      if (!keepFrames) fs.rmSync(framesDir, { recursive: true, force: true });
    }
  } finally {
    cdp?.ws.close();
    chrome.kill("SIGKILL");
    // Chrome 退出时还在往 user-data-dir 里写最后几个文件，紧接着 rmSync 会撞
    // ENOTEMPTY。等一小会儿 + 让 rm 自己重试，比把清理整个跳过干净。
    await sleep(300);
    await new Promise((resolve) => server.close(resolve));
    if (keepFrames) {
      console.log(`临时帧保留在 ${work}`);
    } else {
      fs.rmSync(work, { recursive: true, force: true, maxRetries: 5, retryDelay: 120 });
    }
  }
}

main().catch((err) => {
  console.error(`✗ ${err.message}`);
  process.exit(1);
});
