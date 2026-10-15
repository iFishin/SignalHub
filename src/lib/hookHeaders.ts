/**
 * 额外请求头（`HookAction::Http` 的 `headers`）的编辑规则。
 *
 * 抽成纯函数是因为这两条都容易写错、而且**错了看不出来**：
 *
 * 1. **头名不区分大小写**（HTTP 规范如此）。`Authorization` 和 `authorization` 是
 *    同一个头。允许并存的话，用户在界面上「改完这一行」之后，另一行还在 —— 而两行
 *    里发出去的是哪一个，只有服务器知道。Rust 侧（`config::normalize_headers`）刻意
 *    **不去**合并只差大小写的键：`BTreeMap` 已经丢了用户写下的顺序，谁赢都是任意挑的。
 *    所以这条判断放在**这一层**：顺序还在，能确定地问用户一句。
 * 2. **改名是「删旧键、插新键」两步**。只插不删会留下一个幽灵头，而它照样会被发出去。
 *
 * 这些都比界面上看起来的重要 —— 点一下「发送测试」就真的会发出去，所以「手机上收到
 * 两个一样的头」是这两个错误最终落到用户头上的样子。
 */

/**
 * 请求头条数上限。**必须和 Rust 的 `config::MAX_HOOK_HEADERS` 一致** ——
 * 那边的 clamp 是按**键的字典序**截断的（确定，但用户看不见），所以界面要在这里先
 * 挡住，不然「加了一条，存下去之后它自己没了」会是一个查不出来的现象。
 */
export const MAX_HOOK_HEADERS = 16;

/** 头名归一：去空白。空的（或者只剩空白的）名字 = 这一行还没填。 */
function nameOf(name: string): string {
  return name.trim();
}

/**
 * 这个头名是否已被**另一行**占着。`except` 是正在改的那一行的原键名（新加的行传
 * `null`）—— 不然改自己那一行会被自己挡住。
 */
export function headerTaken(
  headers: Record<string, string>,
  name: string,
  except: string | null = null,
): boolean {
  const wanted = nameOf(name).toLowerCase();
  if (!wanted) return false;
  return Object.keys(headers).some(
    (key) => key.toLowerCase() === wanted && key !== except,
  );
}

function sameHeaders(
  a: Record<string, string>,
  b: Record<string, string>,
): boolean {
  const keys = Object.keys(a);
  if (keys.length !== Object.keys(b).length) return false;
  return keys.every((key) => a[key] === b[key]);
}

/**
 * 提交一行的修改，返回新的一份（**不原地改**）。
 *
 * `from` 是这一行的原键名，`null` = 新加的一行。返回 `null` = **这次修改不合法**
 * （键名和另一行撞了），调用方据此报错并保持原样。
 *
 * 三种情况：
 *
 * - 名字空着、`from` 是已有的一行 → 删掉它。空键在 Rust 那边本来就会被 clamp 丢掉
 *   （HTTP 根本不允许空的头名），留在界面上只会让人以为它真的会被发出去。
 * - 名字空着、新加的一行 → 等于什么都没做（`changed: false`，调用方不必回写设置）。
 * - 其余 → 改了名就删掉旧键，再写入新键（值去空白）。
 */
export function commitHeader(
  headers: Record<string, string>,
  from: string | null,
  draft: { name: string; value: string },
): { headers: Record<string, string>; changed: boolean } | null {
  const name = nameOf(draft.name);
  const next = { ...headers };

  if (name === "") {
    if (from === null) return { headers, changed: false };
    delete next[from];
    return { headers: next, changed: !sameHeaders(headers, next) };
  }

  // **只在真的改了名的时候查重**：没改名的那一行（只是改值）本来就和自己不冲突，
  // 而手写出来的配置里可能本来就有两个只差大小写的键 —— 那种情况下「点了下输入框
  // 又点回来」会莫名其妙弹一句「这个名字已经有了」。
  const renamed = from === null || from !== name;
  if (renamed && headerTaken(headers, name, from)) return null;

  if (from !== null && from !== name) delete next[from];
  next[name] = draft.value.trim();
  return { headers: next, changed: !sameHeaders(headers, next) };
}

/** 删掉一行。 */
export function withoutHeader(
  headers: Record<string, string>,
  name: string,
): Record<string, string> {
  const next = { ...headers };
  delete next[name];
  return next;
}
