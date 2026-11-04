import { useEffect } from "react";
import { DemoStage } from "./DemoStage";
import { useMessages } from "../lib/i18n";
import { inTauri } from "../lib/env";

/**
 * `?window=demo`：一整页的演示，专门给「还没装、先看看」的人，以及 README 动图。
 *
 * 它不是 Tauri 里注册的一个窗口 —— 应用内的演示入口是设置 →「演示」页
 * （同一份 DemoStage，只是缩小一档）。这个路由是纯浏览器页面：拿
 * `npm run dev` 起服务后直接开这条 URL 就能录，不需要跑起整个应用。
 */
export function DemoWindow() {
  const m = useMessages();

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      // 浏览器只允许脚本关掉自己开出来的窗口；关不掉就退回设置页，不留死路。
      window.close();
      if (!inTauri) window.location.href = "?window=settings";
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  return (
    <div className="demo-window">
      <header className="demo-window-head">
        <div className="demo-window-title">
          <strong>{m.demoTitle}</strong>
          <span>{m.demoIntro}</span>
        </div>
        <a className="btn" href="?window=settings">
          {m.demoBackToSettings}
        </a>
      </header>
      <DemoStage />
    </div>
  );
}
