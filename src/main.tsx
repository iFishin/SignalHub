import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { windowKind } from "./lib/env";

// 两个窗口共用一个 bundle，靠 <html> 上的属性给各自的全局样式定锚，
// 这样 CSS 的覆盖关系不依赖打包顺序。
document.documentElement.dataset.window = windowKind();

// 顺序有意为之：主题与配色 tokens 先于用到它们的样式表
import "./styles/theme.css";
import "./styles/state.css";
import "./styles/lamp.css";
import "./styles/settings.css";
import "./styles/inbox.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
