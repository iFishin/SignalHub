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
      '.sign-corner[data-kind="passthrough"]', // 穿透角灯（旧角标删掉后唯一的画法）
      // 0.6.5：一条轨拆成上下两条边，各挂各的画法。少了这两个伪元素，
      // 六行设置里「上边 / 下边」两行会一起变成「点了没反应」。
      ".sign::before",
      ".sign::after",
      '.sign[data-edge-top="off"]::before',
      '.sign[data-edge-bottom="off"]::after',
      '.sign[data-edge-top="glow"]',
      '.sign[data-edge-bottom="fade"]',
      '.dot[data-shape="diamond"]', // 六档灯珠形状里唯一靠 clip-path 画的
      '[data-sign-outline="leaf"]', // 四档牌轮廓
      '[data-effect="sweep"]', // 五档动效
      // 0.6.6：边灯 / 角灯的闪烁。少了 @keyframes 或那两条选择器，
      // 「外观页打开发光项」会变成一次静默无效 —— 开关亮着，灯纹丝不动。
      "@keyframes light-blink",
      ".sign[data-blink-top]::before",
      ".sign-corner[data-blink]",
      "--blink-a", // 闪烁两端的透明度基线，丢了闪起来会抹平明暗档
      "--corner-arc", // 角灯落点跟着圆角算，写死就又会缩回牌内
      "data-sign-light-scale", // 粗细尺子
      "overflow: hidden", // 裁掉超出牌面的内容
    ]) {
      expect(lampCss.includes(needle), `lamp.css 里找不到 ${needle}`).toBe(true);
    }
    // 0.6.4 那条单一轨不能再留：它和新的上下两条边抢同一对伪元素，
    // 两条规则都在的话「只亮上边」会变成「整圈都亮」。
    expect(lampCss.includes('.sign[data-edge="frame"]')).toBe(false);
    // 旧「灵动效果」开关已换成 `data-effect` 五档；留着 `[data-lively]` 的话
    // 预览和真灯会各自读一个不存在的属性，柔光一眼就消失。
    expect(lampCss.includes("[data-lively]")).toBe(false);
    // 穿透那枚常驻 `ban` 角标已经删掉了：CSS 里不该再留「穿透时给 .lamp-flag
    // 换描边色」那条规则（留着的话暂停角标会在穿透时莫名变色，而图标还是暂停）
    expect(lampCss.includes("[data-pass-through] .lamp-flag")).toBe(false);
    // 状态色角灯必须取 `--state` 而不是 `--light-c`：后者在中性档下是灰的，
    // 而「灰的状态点」不表达任何状态（0.6.6 修的就是这个）。整条规则单独取出来
    // 判，避免用到别处（比如注释里）的同名文本。
    const stateCorner =
      /\.sign-corner\[data-kind="state"\]\s*\{[^}]*\}/.exec(lampCss)?.[0] ?? "";
    expect(stateCorner, "lamp.css 里找不到状态色角灯的规则").not.toBe("");
    expect(stateCorner).toContain("var(--state");
    expect(stateCorner).not.toContain("--light-c");
    // 文件以 `}` 收尾：尾巴被吞掉时，最后几行会是一条没结束的注释或声明
    expect(lampCss.trimEnd().endsWith("}")).toBe(true);
  });

  /**
   * 设置页那条子页切换（`LocalTabs`）必须是**分区条**，不能退回成一排小字。
   *
   * 0.6.6 的起因是用户根本没看出「基础 / 质感」是个会换掉大半页内容的开关 ——
   * 它当时是 12.5px 的透明文字。让它退回去只需要删几行样式，而「顺手简化掉
   * 一个看着多余的控件」正是这类回归最常见的来路，所以拿测试钉住那三个特征：
   * 吸顶、等宽铺满、细线轨道。
   */
  it("子页切换是等宽分区条，不是一排链接", () => {
    const start = settingsCss.indexOf(".settings .local-tabs {");
    expect(start, "settings.css 里找不到 .local-tabs").toBeGreaterThan(-1);
    const rule = settingsCss.slice(start, settingsCss.indexOf("}", start));
    // 吸顶：子页比一屏长时，另一边还有一页这件事得一直看得见
    expect(rule).toContain("position: sticky");
    // 细线轨道：有它才像「分区边界」，没它就是一排漂浮的文字
    expect(rule).toContain("border-top: 1px solid var(--line)");

    // 等宽铺满：靠 flex 让每一格自己撑开，而不是按文字长度挤在左边
    const btnStart = settingsCss.indexOf(".settings .local-tabs button {");
    expect(btnStart, "settings.css 里找不到 .local-tabs button").toBeGreaterThan(-1);
    const btn = settingsCss.slice(btnStart, settingsCss.indexOf("}", btnStart));
    expect(btn).toContain("flex: 1 1 0");

    // 每格下面的那行说明（三个子页靠它区分）
    expect(settingsCss).toContain(".settings .local-tabs .tab-hint");
    // 旧的紧凑内边距不能再回来 —— 3px 9px 是页脚链接的分量
    expect(settingsCss.includes("padding: 3px 9px")).toBe(false);
  });

  /**
   * 面板顶部那两个页签的选中态只有一处定义。
   *
   * 和设置面板的 `seg` / 左栏 `rail-item` 是同一条约定：**浅灰底 + 正文字色 +
   * 500 字重**。0.6.6 之前这里根本没有页签（收件箱和留言分在两个地方），所以
   * 「选中态丢了」不会报错、只会在真机上退回成一排分不清的灰字 —— 钉住它。
   */
  it("面板页签的选中态是浅灰底而不是透明文字", () => {
    const active = inboxCss.indexOf(".inbox-tab[data-active] {");
    expect(active, "inbox.css 里找不到 .inbox-tab[data-active]").toBeGreaterThan(-1);
    const rule = inboxCss.slice(active, inboxCss.indexOf("}", active));
    expect(rule).toContain("background: rgba(255, 255, 255, 0.1)");

    // 未读点是蓝的：红在这个窗口里专指「有事在等你」，两种点不能同色
    expect(inboxCss).toContain(".inbox-msg-row[data-unread] .inbox-dot");
    // 正文收起时两行截断 —— 少了它，一条长留言会把整列挤成一面墙
    expect(inboxCss).toContain("-webkit-line-clamp: 2");
  });
});
