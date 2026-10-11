// 把 CLI 编译出来，放到 Tauri 期望的 sidecar 位置。
//
// 为什么要把 CLI 一起装进去：「接入」页写进智能体配置的那条命令就是
// `signalhub hook <agent>`。用户装了 .dmg 却拿不到这个命令的话，一键接入装出来的
// 是个跑不起来的东西 —— 而且它是静默的，不报错（见 hooks.rs 的
// resolve_hook_command）。
//
// 为什么必须有这一步：`bundle.externalBin` 只负责把**已经在那儿**的文件塞进包里，
// 它不会替你编译，而且要求文件名带目标三元组后缀（`signalhub-aarch64-apple-darwin`）。
// 那个名字是 Tauri 用来区分「这份包是给哪个架构的」的，手写不出来 —— 只有构建时
// 才知道。
//
// 由 `beforeBuildCommand` / `beforeDevCommand` 调用，所以能拿到 Tauri 设的环境变量
// `TAURI_ENV_TARGET_TRIPLE` —— 这次在给哪个目标构建（`--target` 传什么就是什么）。
// 直接手跑（`node scripts/stage-cli.mjs`）时没有它，退回本机三元组。
//
// `--debug` 由 dev 那条命令显式传（见 tauri.conf.json）。**不靠环境变量推断** ——
// 这一步的行为差异很大（debug 落在 `target/debug/`，release 落在
// `target/<三元组>/release/`），而「猜错」的表现是 dev 里那个命令找不到，不好查。

import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const manifest = join(root, "src-tauri", "Cargo.toml");

/** 本机三元组。`--print host-tuple` 要 Rust 1.84+，老版本从 `rustc -vV` 里捞。 */
function hostTriple() {
  try {
    return execFileSync("rustc", ["--print", "host-tuple"], {
      encoding: "utf8",
    }).trim();
  } catch {
    const verbose = execFileSync("rustc", ["-vV"], { encoding: "utf8" });
    const host = verbose.match(/^host:\s*(\S+)$/m);
    if (!host) throw new Error("认不出本机的目标三元组");
    return host[1];
  }
}

const triple = process.env.TAURI_ENV_TARGET_TRIPLE?.trim() || hostTriple();
const debug = process.argv.includes("--debug");

// 后缀按**目标**平台判断，不按本机 —— 交叉编译时两者不一样
const exe = triple.includes("windows") ? ".exe" : "";

// dev 时**不带 `--target`**：`tauri dev` 自己也不带，于是 app 落在 `target/debug/`，
// 而 CLI 必须落在同一个目录里才能被 `resolve_hook_command` 按「同目录的兄弟」找到。
// 带 `--target` 会把它挪到 `target/<三元组>/debug/`，那样就找不到了。
//
// 构建时反过来，一律带 `--target`：输出路径因此是确定的，不必去猜 Tauri 这次到底
// 带没带那个参数。
const args = ["build", "--manifest-path", manifest, "--bin", "signalhub"];
if (!debug) args.push("--release", "--target", triple);

// 编译 CLI 时把 `externalBin` 临时置空。
//
// 这一步是**必须的**，否则会卡在一个先有鸡还是先有蛋里：编译这个包里的任何东西
// （包括 CLI 自己）都会跑 `build.rs`，而 `tauri-build` 在**构建脚本阶段**就校验
// `bundle.externalBin` 里的文件存不存在 —— 于是「编译那个文件」的命令本身要求
// 那个文件已经存在。
//
// `TAURI_CONFIG` 是 Tauri 用来覆盖配置的 JSON，合并进 `tauri.conf.json`。这里只把
// 那一个键置空，别的一概不动。
const noSidecar = JSON.stringify({ bundle: { externalBin: [] } });

execFileSync("cargo", args, {
  stdio: "inherit",
  env: { ...process.env, TAURI_CONFIG: noSidecar },
});

const profile = debug ? "debug" : "release";
const from = debug
  ? join(root, "src-tauri", "target", profile, `signalhub${exe}`)
  : join(root, "src-tauri", "target", triple, profile, `signalhub${exe}`);
const to = join(root, "src-tauri", "binaries", `signalhub-${triple}${exe}`);

mkdirSync(dirname(to), { recursive: true });
copyFileSync(from, to);
console.log(`[stage-cli] ${to}`);
