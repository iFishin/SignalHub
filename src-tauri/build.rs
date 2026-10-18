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
    // 所以只给测试目标补一条链接参数。用 `-tests` 而不是无后缀的
    // `rustc-link-arg`：正式二进制已经有 winres 那一份，重复声明同一个依赖
    // 没有意义，也可能让链接器抱怨。
    #[cfg(all(windows, target_env = "msvc"))]
    println!(
        r#"cargo:rustc-link-arg-tests=/MANIFESTDEPENDENCY:"type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'""#
    );

    tauri_build::build()
}
