/**
 * 「打开设置，并且直接翻到某一页」。
 *
 * `open_settings` 是个不带参数的命令 —— 设置窗在应用启动时就已经以
 * `visible: false` 建好（见 `useVisibleRefresh` 的注释），命令只负责 `show()`，
 * 所以「翻到哪一页」没法走命令参数。同一份 localStorage 是同源的两个窗口唯一
 * 共享的通道，于是用它递一张**一次性纸条**：写的一方在打开之前写，读的一方读到
 * 就翻页并立刻删掉。
 *
 * **为什么读的时机不能只在挂载时**：设置窗的组件整个应用生命周期只挂载一次
 * （启动那一刻），而纸条是在用户点「接入」/「看演示」时才写的。所以设置面板除了
 * 初始化读一次，还要在窗口每次重新显形（`focus`）时再读一次 —— 这和
 * `useVisibleRefresh` 处理「只在 mount 取一次」那类数据用的是同一个判据。
 *
 * 键只在这个文件里出现。早先它写在灯组件里、由设置面板反向 import，两处真值
 * 迟早会漂；现在写的一方、读的一方都只认这里的常量。
 */

/** 纸条的键。值是设置面板的页签 id（`connect` / `demo` / `history` …）。 */
export const OPEN_TAB_KEY = "signalhub.settings.openTab";

/**
 * 「翻到某一页」的**第二条通道**：Tauri 事件。
 *
 * 纸条只能由前端的另一个窗口写，而有些入口在 **Rust** 那边 —— 灯上右键菜单的
 * 「重命名…」就是（菜单是原生的，见 `menu.rs`）。那边写不了 localStorage，只好
 * 由它 `emit` 一个事件，设置窗在这里收。
 *
 * 两条通道并存而不是二选一：纸条是「同源窗口之间递东西」的做法（灯窗 → 设置窗
 * 都在前端），事件是「Rust → 前端」的做法。用事件去做前者要多绕一层 IPC，
 * 用纸条去做后者根本做不到。
 */
export const OPEN_TAB_EVENT = "signalhub://settings-tab";

/** 写纸条。存储被禁用时安静失败 —— 设置照常打开，只是停在「上次那页」。 */
export function requestSettingsTab(tab: string) {
  try {
    window.localStorage.setItem(OPEN_TAB_KEY, tab);
  } catch {
    /* 隐私模式：翻不了页不影响用 */
  }
}

/**
 * 读纸条。**不删** —— 同一个值会被读两次（组件初始化和第一次 `focus`），
 * 删掉的话第二次就落回「上次那页」，表现成「点了接入却停在状态页」。
 * 删由 `clearSettingsTab` 负责。
 */
export function readSettingsTab(): string | null {
  try {
    return window.localStorage.getItem(OPEN_TAB_KEY);
  } catch {
    return null;
  }
}

export function clearSettingsTab() {
  try {
    window.localStorage.removeItem(OPEN_TAB_KEY);
  } catch {
    /* 删不掉不影响使用，只是下次可能再翻一次 */
  }
}
