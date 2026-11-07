import { useEffect, useRef, useState } from "react";
import { motion, useReducedMotion } from "motion/react";
import { useMessages } from "../lib/i18n";
import type { SignalState } from "../lib/types";

/**
 * 长按灯呼出的**圆环菜单**。
 *
 * 为什么是圆环而不是原生菜单：右键那张（`menu.rs`）是「这一组的操作」——
 * 改名 / 静音 / 隐藏 / 手动状态，项多且要分父子。圆环管的是**灯本身的动作**，
 * 只有四件、彼此平级，正好绕一圈放下；而且圆环能带中心那颗灯珠，让「我在操作
 * 这盏灯」这件事一直看得见。
 *
 * 窗口尺寸由 Rust 换成正方形（`config::menu_window_size`）—— 圆环画在灯牌外面，
 * 窗口不放大就会被 `overflow: hidden` 裁掉。这里只管画，不管尺寸。
 */

/** 一项菜单。`icon` 用字符而不是 SVG：这个窗口不加载图标库，字符跨平台最省。 */
export interface MenuItemSpec {
  id: string;
  icon: string;
  label: string;
  onSelect: () => void;
  /** 退出这类不可逆的动作，悬停时用警示色 */
  danger?: boolean;
}

interface Props {
  items: MenuItemSpec[];
  /** 中心那颗灯珠的状态 —— 圆环的视觉锚点，也是「你正在操作哪盏灯」的答案 */
  aggregate: SignalState;
  onClose: () => void;
}

/**
 * 第 `index` 项（共 `count` 项）该放在哪个角度。
 *
 * 从正上方（-90°）开始顺时针均分。**从上方开始**是因为四项时正好落在
 * 上 / 右 / 下 / 左，和「读一圈」的直觉一致；从右侧开始的话第一项会落在
 * 最容易和灯牌右边距混淆的位置。
 */
function angleOf(index: number, count: number): number {
  return -90 + (360 / count) * index;
}

export function LampMenu({ items, aggregate, onClose }: Props) {
  const m = useMessages();
  const reduceMotion = useReducedMotion();
  const count = items.length;
  const itemRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const [active, setActive] = useState(0);

  // 打开就把焦点交给第一项。之前 `role="menu"` 只有 Esc 能用，方向键会走到哪一项
  // 全靠猜 —— 现在它是一圈真的能用键盘走的菜单。
  useEffect(() => {
    itemRefs.current[0]?.focus();
  }, []);

  // Esc 关闭（和设置窗 / 收件箱对齐），方向键沿圆环走，数字键 1–4 直达。
  // 监听挂在 window 而不是菜单容器上：灯窗是 `focus: false` 的常驻小窗，
  // 万一焦点没能落到按钮上，方向键也得能走。Enter / Space 交给按钮自己处理。
  useEffect(() => {
    const focusAt = (index: number) => {
      setActive(index);
      itemRefs.current[index]?.focus();
    };
    const onKeyDown = (event: KeyboardEvent) => {
      switch (event.key) {
        case "Escape":
          event.stopPropagation();
          onClose();
          return;
        case "ArrowRight":
        case "ArrowDown":
          focusAt((active + 1) % count);
          break;
        case "ArrowLeft":
        case "ArrowUp":
          focusAt((active - 1 + count) % count);
          break;
        case "Home":
          focusAt(0);
          break;
        case "End":
          focusAt(count - 1);
          break;
        default: {
          const digit = Number(event.key);
          if (!Number.isInteger(digit) || digit < 1 || digit > count) return;
          focusAt(digit - 1);
        }
      }
      event.preventDefault();
      event.stopPropagation();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [active, count, onClose]);

  return (
    <div
      className="lamp-menu"
      role="menu"
      aria-label={m.lampMenuLabel}
      aria-activedescendant={`lamp-menu-item-${items[active]?.id ?? ""}`}
    >
      {/* 点击空白处关闭。铺满整个窗口，所以圆环之间的空隙也能点中。
          它在菜单项**之前**，靠 z-index 落在下面（见 CSS）。 */}
      <button
        type="button"
        className="lamp-menu-scrim"
        aria-label={m.lampMenuCloseHint}
        title={m.lampMenuCloseHint}
        onClick={onClose}
      />

      {/* 中心：当前状态那颗灯珠。点它也是关闭 —— 手已经在中心了，
          往回点一下比去找空白处更顺手。 */}
      <motion.button
        type="button"
        className="lamp-menu-core"
        data-state={aggregate}
        title={m.lampMenuCloseHint}
        aria-label={m.lampMenuCloseHint}
        onClick={onClose}
        initial={reduceMotion ? false : { scale: 0.7, opacity: 0 }}
        animate={{ scale: 1, opacity: 1 }}
        transition={
          reduceMotion
            ? { duration: 0 }
            : { type: "spring", stiffness: 420, damping: 26, delay: 0.04 }
        }
      >
        <span className="dot lamp-menu-dot" data-state={aggregate} data-shape="circle" />
      </motion.button>

      {items.map((item, index) => {
        const angle = angleOf(index, count);
        const rad = (angle * Math.PI) / 180;
        // 位置由 CSS 变量给：`--menu-x` / `--menu-y` 是单位圆上的坐标，
        // CSS 再乘 `--menu-radius` 摆到圆环上 —— 半径只有那一个真值
        // （和 Rust 的 MENU_RADIUS 对齐，见 config.rs）。
        //
        // **定位在 `slot` 上，动画在里面的 `button` 上**：motion 的 scale 走的是
        // 元素自己的 `transform`，如果定位也靠 transform，两者会互相覆盖
        // （scale 会把 translate 顶掉，菜单项全挤到中心）。
        const style = {
          "--menu-x": Math.cos(rad).toFixed(4),
          "--menu-y": Math.sin(rad).toFixed(4),
        } as React.CSSProperties;

        return (
          <span key={item.id} className="lamp-menu-slot" style={style}>
            <motion.button
              type="button"
              role="menuitem"
              id={`lamp-menu-item-${item.id}`}
              ref={(node) => {
                itemRefs.current[index] = node;
              }}
              tabIndex={index === active ? 0 : -1}
              onFocus={() => setActive(index)}
              className="lamp-menu-item"
              data-danger={item.danger || undefined}
              title={item.label}
              aria-label={item.label}
              onClick={() => {
                // **先关再执行**：动作可能打开另一个窗口（收件箱 / 设置），
                // 而菜单窗口缩回原尺寸这件事要在那之前发生 —— 否则用户会看到
                // 圆环和收件箱同时出现一下。
                onClose();
                item.onSelect();
              }}
              initial={reduceMotion ? false : { scale: 0.3, opacity: 0 }}
              animate={{ scale: 1, opacity: 1 }}
              transition={
                reduceMotion
                  ? { duration: 0 }
                  : {
                      type: "spring",
                      stiffness: 380,
                      damping: 22,
                      // 依次弹出：从中心往外一圈，像灯珠把选项「甩」出去
                      delay: 0.05 + index * 0.035,
                    }
              }
            >
              <span className="lamp-menu-icon" aria-hidden="true">
                {item.icon}
              </span>
              <span className="lamp-menu-label">{item.label}</span>
            </motion.button>
          </span>
        );
      })}
    </div>
  );
}
