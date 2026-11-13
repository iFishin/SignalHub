import { describe, expect, it } from "vitest";
import lampCss from "./lamp.css?raw";
import settingsCss from "./settings.css?raw";
import stateCss from "./state.css?raw";
import inboxCss from "./inbox.css?raw";
import demoCss from "./demo.css?raw";
import settingsPanelSource from "../components/SettingsPanel.tsx?raw";

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
      ".sign-corner", // 行尾四格微记号
      '.sign-corner[data-kind="passthrough"]', // 穿透角灯（旧角标删掉后唯一的画法）
      // 0.6.5：一条轨拆成上下两条边，各挂各的画法。少了这两个伪元素，
      // 六行设置里「上边 / 下边」两行会一起变成「点了没反应」。
      ".sign::before",
      ".sign::after",
      '.sign[data-edge-top="off"]::before',
      '.sign[data-edge-bottom="off"]::after',
      '.sign[data-edge-top="glow"]',
      '.sign[data-edge-bottom="fade"]',
      // 0.6.7：上下两条边只落在牌的**首行上沿 / 末行下沿**。原来六个槽每行都画，
      // 于是多行牌中间每道分隔线两侧各压着一条 —— 上一行的「下」和下一行的「上」，
      // 颜色跟各自状态走、长度跟各自档走，看起来就是两截不同颜色的短线粘在同一道
      // 缝上。只有一行时看不出来，而设置页的预览恒是两行。删掉这两条不报错，
      // 只是那条「双色双线」会回来。
      ".sign-board > .sign:not(:first-child)::before",
      ".sign-board > .sign:not(:last-child)::after",
      '.dot[data-shape="diamond"]', // 六档灯珠形状里唯一靠 clip-path 画的
      '[data-sign-outline="leaf"]', // 四档牌轮廓
      '[data-effect="sweep"]', // 五档动效
      // 0.6.6：边灯 / 角灯的闪烁。少了 @keyframes 或那两条选择器，
      // 「外观页打开发光项」会变成一次静默无效 —— 开关亮着，灯纹丝不动。
      "@keyframes light-blink",
      ".sign[data-blink-top]::before",
      ".sign-corner[data-blink]",
      "--blink-a", // 闪烁两端的透明度基线，丢了闪起来会抹平明暗档
      // 0.6.7：多组共享一块板。少了这层，就会退回「几张独立卡片 + 11px 断口」，
      // 而 CSS 不报错、只是看着散。
      ".sign-board {",
      ".sign-board > .sign:not(:last-child)", // 行距消化在每行底部，窗口高度公式不动
      "--mark-stack-h", // 行尾四格竖排一列
      // 0.6.7：折叠形态两轴。少了 `.fold-board` 或格宽变量，「折叠内容」
      // 四档里后三档会静默退化成裸点，窗口却仍按窄板尺寸开着。
      ".fold-board {",
      ".fold-row {",
      "--fold-cell-w",
      '.fold-board[data-axis="horizontal"]',
      "text-overflow: ellipsis", // 长组名不能撑破固定格宽
      ".fold-board .lamp-dot-more", // 板里那枚 +N 不叠第二层牌底
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
    // 0.6.7 的形状词汇：圆只归主信号灯。旧的四角落点、整条彩边、发光和渐变
    // 一样都不该回来 —— 这些全是「静默变丑」，没有报错。
    expect(lampCss.includes("--corner-arc")).toBe(false);
    expect(lampCss.includes("mask-image")).toBe(false);
    expect(lampCss.includes("border-top-color: transparent")).toBe(false);
    // `.sign-corner` 本身不能是圆：方 / 短横 / 竖条才是记号
    const cornerRule = /\.sign-corner\s*\{[^}]*\}/.exec(lampCss)?.[0] ?? "";
    expect(cornerRule, "lamp.css 里找不到角灯的基础规则").not.toBe("");
    expect(cornerRule).not.toContain("border-radius: 50%");
    // 角灯列不能再减一次行距。行距是上下各半加进 padding 的，行中心没动；
    // 再减一次会让非末行整列偏上半个行距（22px 档 5.5px），一块两行的牌看起来
    // 就是「上面那枚浮在行的上半截、下面那枚居中」。只有一行时没有行距，
    // 所以这个偏差一直没被发现 —— 去掉它不报错，只是角灯又歪。
    expect(cornerRule).not.toContain("row-pad");
    // 刻线必须**水平居中**：`left/right: 0` + `margin-inline: auto`，按整牌的宽算。
    // 它原先是 `left: calc(var(--size) * 0.36)` —— 那是**文字**的左边距，当初只是
    // 为了躲板的圆角才比进来，顺带把参照系也换错了：边上的记号去对齐文字起点，
    // 于是它既不是边框光、也不是对称记号，读起来就是「一条悬在左上方的短线」。
    // 改回左对齐不会报错、也不会裁掉任何东西，只是又不居中了。
    const tickRule = /\.sign::before,\s*\.sign::after\s*\{[^}]*\}/.exec(lampCss)?.[0] ?? "";
    expect(tickRule, "lamp.css 里找不到刻线的基础规则").not.toBe("");
    expect(tickRule).toContain("left: 0");
    expect(tickRule).toContain("right: 0");
    expect(tickRule).toContain("margin-inline: auto");
    expect(tickRule).not.toContain("0.36");
    // 状态色角灯必须取 `--state` 而不是 `--light-c`：后者在中性档下是灰的，
    // 而「灰的状态点」不表达任何状态（0.6.6 修的就是这个）。整条规则单独取出来
    // 判，避免用到别处（比如注释里）的同名文本。
    const stateCorner =
      /\.sign-corner\[data-kind="state"\]\s*\{[^}]*\}/.exec(lampCss)?.[0] ?? "";
    expect(stateCorner, "lamp.css 里找不到状态色角灯的规则").not.toBe("");
    expect(stateCorner).toContain("var(--state");
    expect(stateCorner).not.toContain("--light-c");
    // 板必须**按内容宽**：真窗口里 `100%` 与 `max-content` 等价（窗口宽恰好是
    // 牌宽 + `LAMP_MARGIN`），可设置页的预览把 `.lamp-root` 放进一块宽画布 ——
    // 写成 `100%` 时板被拉成一面墙、行还停在 154px，右边留一片空的深色，
    // 用户看到的就是「预览里的牌比真灯胖一截」。改回去不会报错，只会静默变样。
    const boardRule = /\.sign-board\s*\{[^}]*\}/.exec(lampCss)?.[0] ?? "";
    expect(boardRule, "lamp.css 里找不到 .sign-board 的基础规则").not.toBe("");
    expect(boardRule).toContain("width: max-content");
    expect(boardRule).not.toContain("width: 100%");
    // 文件以 `}` 收尾：尾巴被吞掉时，最后几行会是一条没结束的注释或声明
    expect(lampCss.trimEnd().endsWith("}")).toBe(true);
  });

  /**
   * 折叠默认竖排：这一条是用户明确要的默认改动。横排必须由
   * `[data-fold-axis="horizontal"]` 显式覆盖 —— 把基础规则改回 `row` 的话
   * 不会报错，只是所有升级上来的人又看到那条长条。
   */
  it("折叠默认竖排，横排是显式的一档", () => {
    const start = lampCss.indexOf('.lamp-root[data-collapsed="true"] {');
    expect(start, "lamp.css 里找不到折叠态基础规则").toBeGreaterThan(-1);
    const rule = lampCss.slice(start, lampCss.indexOf("}", start));
    expect(rule).toContain("flex-direction: column");
    expect(rule).not.toContain("flex-direction: row");
    expect(lampCss).toContain(
      '.lamp-root[data-collapsed="true"][data-fold-axis="horizontal"]',
    );
    // 点距走整数 px 变量，不能退回小数 calc —— 那是旧折叠态差半像素的来源
    expect(rule).toContain("var(--fold-gap");
    expect(rule).not.toContain("calc(var(--size) / 3)");
    // 带文字的窄板在设置页有实时预览，不能只剩孤零零的一行 seg
    expect(settingsCss).toContain(".settings .fold-preview");
  });

  /**
   * 折叠板是**一格宽的窄板**，不能直接套展开牌那套尺寸。
   *
   * 用户报的「折叠起来左边有个边框、间隙很大」，根因是牌轮廓（叶片 / 拱形 /
   * 胶囊）的圆角按**牌宽**取到 0.62–0.75×size（13–16px），而折叠板行高只有
   * 1.5×size、灯珠左边距 5px —— 那道弧横跨第一行的整个左上角，看起来就像板
   * 缺了一块、灯珠被削掉。三条不变量，改回去都不报错、只是静默变丑：
   * ① 折叠板圆角封在「标准」档；② 轮廓那两条大圆角只管 `.sign-board`；
   * ③ 带文字的格里灯珠按 0.8×size 画，不满格。
   */
  it("折叠板按自己的尺寸收圆角，不跟展开牌", () => {
    const start = lampCss.indexOf(".fold-board {");
    expect(start, "lamp.css 里找不到 .fold-board").toBeGreaterThan(-1);
    const rule = lampCss.slice(start, lampCss.indexOf("}", start));
    // 封顶：min(用户圆角倍率, 0.32)。少了 min 的话「圆角 → 大」会把灯珠包进弧里
    expect(rule).toContain("min(var(--sign-radius");
    expect(rule).toContain("0.32");
    // 轮廓的大圆角只作用于展开牌；`:is(.sign-board, .fold-board)` 会把它也套上去
    expect(lampCss).toContain('.lamp-root[data-sign-outline="leaf"] .sign-board');
    expect(lampCss).not.toContain(':is(.sign-board, .fold-board)');
    // 带文字的格：灯珠 0.8×size。满格（--size）时它会占掉行高的三分之二
    expect(lampCss).toContain(".fold-row > .lamp-dot");
    expect(lampCss).toContain("--dot: calc(var(--size) * 0.8)");
  });

  /**
   * 预览**是一张居中的图，不是一块样品台**。
   *
   * 这块位置被报过三次，每次都是「换个方向过头」：
   * ① `--hover` 灰底 + `min-width: 240px` + 居中 —— 牌 154px 摆在 240px 灰块
   *    中间，左右各留五十多像素，用户说「缩起来的信号灯左侧有个空隙」「把界面
   *    改乱了」；
   * ② 去掉灰底之后改成左对齐 —— 牌贴着左边缘、右边空五百多像素，视觉重心全歪，
   *    用户又说「预览没有居中」。
   *
   * 正确解是两条**分开**的：居中是几何（`margin-inline: auto`），不能借色块去
   * 「托住」它。四条不变量，改回去都不报错、只是静默变丑：
   * ① `width: fit-content`（铺满整列会变成一面墙）；
   * ② 水平居中（去掉 auto 就退回「贴左边、右边一大片空」）；
   * ③ 不画底色 / 圆角 / 最小宽度（那就是又摆回一块灰样品台）；
   * ④ 「折叠后」那行图注跟着居中（板居中、字左对齐会差出半个屏）。
   */
  it("预览按内容宽居中，不画样品台", () => {
    const selector = ".settings .lamp-preview,";
    const start = settingsCss.indexOf(selector);
    expect(start, "settings.css 里找不到预览那条合并规则").toBeGreaterThan(-1);
    const rule = settingsCss.slice(start, settingsCss.indexOf("}", start));
    expect(rule).toContain("width: fit-content");
    expect(rule).toContain("margin: 0 auto 6px");
    expect(rule).not.toContain("min-width");
    expect(rule).not.toContain("background");
    expect(rule).not.toContain("border-radius");

    // 折叠预览上方那行「折叠后」小字：没有它，窄板会紧贴「折叠内容」那一行的
    // 说明文字，看起来像同一段说明的第二行；不居中则和窄板差出半个屏。
    const titleStart = settingsCss.indexOf(".settings .fold-preview-title {");
    expect(titleStart, "找不到 .fold-preview-title").toBeGreaterThan(-1);
    const titleRule = settingsCss.slice(titleStart, settingsCss.indexOf("}", titleStart));
    expect(titleRule).toContain("text-align: center");
  });

  /**
   * 折叠预览**不能被「形态」门控**。
   *
   * 「组名 / 序号 / 状态」三档只在折叠态生效，所以上一版把它写成
   * `{settings.collapsed && <FoldPreview/>}`。后果是：展开态下点这三档，按钮只
   * 换一个高亮、真灯一个像素不动、预览整块不出现 —— 用户读成「选择都选择不了」。
   *
   * 这条读的是 TSX 源文本（不是 CSS）：它钉的是「这一块必须常驻」这条呈现约定，
   * 用 `?raw` 比渲染整页设置面板便宜得多。改回门控不会报错，只会让那三档重新
   * 变回「点了没反应」。
   */
  it("折叠预览恒常显示，不跟形态走", () => {
    expect(settingsPanelSource).toContain("<FoldPreview settings={settings} />");
    expect(settingsPanelSource).not.toMatch(/settings\.collapsed\s*&&\s*<FoldPreview/);
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
