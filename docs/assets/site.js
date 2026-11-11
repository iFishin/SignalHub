/*
 * SignalHub 介绍页脚本 —— 中英两页共用。
 *
 * 只做三件小事，刻意不做路由 / 框架 / 构建：
 *   1. 顶栏滚过 8px 才出现那条 1px 分隔线；
 *   2. 章节进入视口时淡入几像素（入场用的 .armed 由本文件加，
 *      脚本没跑起来时内容仍然完整可见）；
 *   3. Hero 里那三块牌按剧本轮播状态 —— 展示的是产品本身的核心机制
 *      「同一盏灯里，每块牌各说各的状态」，不是装饰性动画。
 *
 * 状态词两页各一份，按 <html lang> 选；其余剧本（状态、组名、时长、
 * 哪一块更响）两页完全相同，避免中英页讲成两个产品。
 * 常驻动画只动 opacity，和 lamp.css 的约束一致；系统要求减少动态效果时
 * 一律停在剧本第 0 帧，也就是 HTML 里静态写好的那一帧。
 */
(function () {
  "use strict";

  var reduce = window.matchMedia &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  var en = document.documentElement.lang === "en";

  var top = document.getElementById("top");
  var onScroll = function () {
    if (top) top.classList.toggle("stuck", window.scrollY > 8);
  };
  window.addEventListener("scroll", onScroll, { passive: true });
  onScroll();

  var reveals = Array.prototype.slice.call(document.querySelectorAll(".reveal"));
  if (!reduce && "IntersectionObserver" in window && reveals.length) {
    reveals.forEach(function (el) { el.classList.add("armed"); });

    // 先同步放出「这一刻就在视口里」的那些：观察器回调是异步的，静态截图、
    // file:// 直开、个别 WebKit 环境里它可能压根不回调。内容绝不能因为一段
    // 装饰性动画而永久停在 opacity: 0 —— 这条底线比动画本身重要。
    reveals.forEach(function (el) {
      var box = el.getBoundingClientRect();
      if (box.top < window.innerHeight && box.bottom > 0) el.classList.add("in");
    });

    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (entry) {
        if (!entry.isIntersecting) return;
        entry.target.classList.add("in");
        io.unobserve(entry.target);
      });
    }, { rootMargin: "0px 0px -40px 0px", threshold: 0.08 });
    reveals.forEach(function (el) { io.observe(el); });

    // 最后一道兜底：无论观察器有没有回调，2.5 秒后一律可见。
    setTimeout(function () {
      reveals.forEach(function (el) { el.classList.add("in"); });
    }, 2500);
  }

  var labels = en
    ? {
        idle: "Idle", thinking: "Thinking", working: "Working",
        done: "Done", attention: "Attention", permission: "Permission",
        error: "Error", stale: "Stale", offline: "Offline", off: "Paused"
      }
    : {
        idle: "空闲", thinking: "推理", working: "执行",
        done: "完成", attention: "关注", permission: "授权",
        error: "出错", stale: "沉默", offline: "离线", off: "暂停"
      };

  var SCRIPT = [
    [
      { name: "api",   state: "working",    age: "02:14", tone: "quiet" },
      { name: "build", state: "permission", age: "00:12", tone: "loud" },
      { name: "docs",  state: "done",       age: "00:41", tone: "quiet" }
    ],
    [
      { name: "api",   state: "working", age: "02:31", tone: "quiet" },
      { name: "build", state: "working", age: "00:03", tone: "quiet" },
      { name: "docs",  state: "done",   age: "00:58", tone: "quiet" }
    ],
    [
      { name: "api",   state: "thinking", age: "00:04", tone: "quiet" },
      { name: "build", state: "working",  age: "00:19", tone: "quiet" },
      { name: "docs",  state: "stale",    age: "12:06", tone: "quiet" }
    ],
    [
      { name: "api",   state: "error",   age: "00:07", tone: "loud" },
      { name: "build", state: "working", age: "00:35", tone: "quiet" },
      { name: "docs",  state: "stale",   age: "12:23", tone: "quiet" }
    ]
  ];

  var signs = Array.prototype.slice.call(
    document.querySelectorAll(".lamp-demo .sign")
  );
  if (!signs.length) return;

  var apply = function (frame) {
    frame.forEach(function (row, i) {
      var sign = signs[i];
      if (!sign) return;
      sign.setAttribute("data-state", row.state);
      sign.setAttribute("data-tone", row.tone);
      var name = sign.querySelector(".sign-name");
      var state = sign.querySelector(".sign-state");
      var age = sign.querySelector(".sign-age");
      if (name) name.textContent = row.name;
      if (state) state.textContent = labels[row.state];
      if (age) age.textContent = row.age;
    });
  };

  // HTML 里静态写的就是第 0 帧：无 JS / 减少动态效果时都不需要补画。
  apply(SCRIPT[0]);
  if (reduce) return;

  var at = 0;
  setInterval(function () {
    at = (at + 1) % SCRIPT.length;
    apply(SCRIPT[at]);
  }, 2800);
})();
