import { describe, expect, it } from "vitest";
import lampCss from "./lamp.css?raw";
import settingsCss from "./settings.css?raw";
import stateCss from "./state.css?raw";
import inboxCss from "./inbox.css?raw";
import demoCss from "./demo.css?raw";

/**
 * 样式表的两条「静默失效」防线。
 *
 * 起因是一次真实事故：在 `lamp.css` 里加一节新样式时**忘了闭合注释**，于是后面
 * 三条规则（`.sign-state` / `.sign-message` / `.sign-age`）被整段吞掉。表现是
 * 牌上的状态词竖着排、消息折成两行、字被裁掉 —— 而**没有任何报错**：CSS 解析器
 * 遇到未闭合的注释只会一路吃到文件末尾（或下一个闭合标记），谁也不告诉我们。
 *
 * 这类故障和项目里别的「两处真值」是同一个性质：改错了不报错，只会在真机上
 * 难看。所以拿测试兜住它 —— 成本是一行 `?raw` 导入。
 */
const SHEETS: [string, string][] = [
  ["lamp.css", lampCss],
  ["settings.css", settingsCss],
  ["state.css", stateCss],
  ["inbox.css", inboxCss],
  ["demo.css", demoCss],
];

describe("样式表", () => {
  /** 注释开闭必须配平。少一个会吞掉后面的规则，多一个会把规则变成垃圾。 */
  it("每条注释都闭合了", () => {
    for (const [name, css] of SHEETS) {
      const opens = (css.match(/\/\*/g) ?? []).length;
      const closes = (css.match(/\*\//g) ?? []).length;
      expect(closes, `${name} 的注释没闭合（多出或缺少闭合标记）`).toBe(opens);
    }
  });

  /**
   * 「牌面」那几条**不能被删掉**的规则。
   *
   * 不是给重构设障碍，而是这几处一旦消失，表现是「文字换行、牌被撑高、最后一块
   * 被 `overflow: hidden` 裁掉」—— 和 Rust 算出来的牌高对不上，而界面上只表现为
   * 「有点挤」，很难联想到是 CSS 没了。
   */
  it("牌面的关键规则还在", () => {
    for (const needle of [
      "white-space: nowrap", // 组名 / 消息不换行（靠省略号截断）
      ".sign-corner", // 四角小灯
      '.sign[data-edge="frame"]', // 边灯那条轨（按牌挂，不是挂在窗口根上）
      "--corner-arc", // 角灯落点跟着圆角算，写死就又会缩回牌内
      "data-sign-light-scale", // 粗细尺子
      "overflow: hidden", // 裁掉超出牌面的内容
    ]) {
      expect(lampCss.includes(needle), `lamp.css 里找不到 ${needle}`).toBe(true);
    }
    // 文件以 `}` 收尾：尾巴被吞掉时，最后几行会是一条没结束的注释或声明
    expect(lampCss.trimEnd().endsWith("}")).toBe(true);
  });
});
