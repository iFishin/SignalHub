fn main() {
    // Windows 的**测试二进制**载不动（0xc0000139 STATUS_ENTRYPOINT_NOT_FOUND），
    // 这个坑值得写清楚，因为它看起来像环境问题、其实是链接期就注定的：
    //
    // - `tauri` 的默认特性里有 `common-controls-v6`，于是 `tauri-runtime-wry` 的
    //   对话框直接 import `TaskDialogIndirect`；
    // - 这个函数**只在 comctl32 v6 里**，而 `System32\comctl32.dll` 是最老的
    //   5.82（`dumpbin /exports` 里查不到它）；要绑到 v6，exe 必须声明对
    //   `Microsoft.Windows.Common-Controls 6.0.0.0` 的依赖；
    // - 正式产物由 tauri-build / tauri-winres 把 manifest 编进资源段，
    //   **测试二进制没有那一段**（`dumpbin /headers` 里看不到 .rsrc），
    //   于是加载器按无 manifest 处理 → 绑 5.82 → 找不到入口点。
    //
    // 所以给链接器补一条 manifest 依赖。两个细节都是踩出来的：
    //
    // 1. **不能用 `rustc-link-arg-tests`。** 它的「tests」指 `tests/` 目录下那种
    //    集成测试目标；本包的测试全在 `src/` 里的 `#[cfg(test)] mod tests`，
    //    属于 lib 的**单元测试**目标。包里根本没有集成测试时 Cargo 会直接报
    //    「does not have a test target」让构建失败（这条先上过 CI，整包编不过）；
    //    就算放一个进去，实测它也只作用于那个集成测试，单元测试照样崩。
    //    所以只能用 `rustc-link-arg`（作用于 bins / tests 等全部可执行目标）。
    // 2. **只在非 release 下加。** 正式产物的 manifest 由 winres 嵌进资源段、
    //    本来是对的；再叠一条依赖属于「没验过」的改动，而 Windows 的正式包在
    //    macOS 上验不了、CI 也只在打 tag 时才构建。收窄到 debug / test，
    //    CI 和本地 `cargo test` 都受益，出货的东西一行没变。
    //    （代价：`cargo test --release` 在 Windows 上仍会踩这个坑，别那么跑就行。）
    //
    // 值里**不要再套一层双引号**。MS 文档给的例子是 `/MANIFESTDEPENDENCY:"type=…"`，
    // 那是写在 .vcxproj / 命令行里的形态；走 rustc 时它会替我们把这个参数整体包一
    // 层引号（因为里面有空格），而内层那对双引号会被 link.exe 当成参数边界，报
    // `LNK1181: cannot open input file 'name='Microsoft.Windows.Common-Controls''`。
    #[cfg(all(windows, target_env = "msvc"))]
    if std::env::var("PROFILE").as_deref() != Ok("release") {
        println!(
            "cargo::rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }

    tauri_build::build()
}
