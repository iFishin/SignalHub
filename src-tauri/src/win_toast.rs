//! Windows 的系统通知身份（AppUserModelID / AUMID）。
//!
//! **为什么会有这个文件**：Windows 的 Toast 不是「谁调用就归谁」，而是发给一个「应用
//! 身份」。`tauri-plugin-notification` 在 Windows 上把通知的 `app_id` 固定设成
//! `tauri.conf.json` 的 `identifier`（本仓库是 `app.signalhub.desktop`），而
//! `ToastNotificationManager::CreateToastNotifierWithId(id)` 要求这个 id 已经由
//! **开始菜单里带 `System.AppUserModel.ID` 属性的快捷方式**注册过。Tauri 的 NSIS
//! 安装器会盖这个章，所以 `-setup.exe` 装的版本能弹；portable zip 没有安装器，
//! 于是 Toast 被系统直接丢掉 —— 不报错、不响、界面还显示「已允许」。
//!
//! 这里在启动时自己把这套身份补齐，**只写当前用户的两处**：一个 `HKCU` 键（决定通知
//! 来源显示成 SignalHub 而不是一串 id）和一个开始菜单 `.lnk`（决定系统肯不肯发）。
//! 不需要管理员、不写 Program Files、不装服务。只写注册表**不够** —— 微软
//! WindowsAppSDK#6821 里「注册成功、权限是 Enabled、就是没有横幅」正是那种情况，
//! 决定性的因素是快捷方式上的属性。
//!
//! **图标必须是图片文件，不能是 exe。** AUMID 键里的 `IconUri` 只认 png / jpg；
//! 指向 exe 时通知照常弹，只是来源那一栏只有名字、没有图标 —— 0.6.2 的版本写的就是
//! exe，用户看到的现象是「Windows 通知没有图标」。现在把 `icons/128x128.png`
//! `include_bytes!` 进二进制、首次注册时落到 `~/.signalhub/toast-icon.png`，再把那个
//! 路径写进注册表。这条路和 `Microsoft.Toolkit.Uwp.Notifications` 对 unpackaged
//! 应用的做法一致（它也是把图标解成 `Icon.png` 再写路径）。
//!
//! 上游依据：
//! - <https://learn.microsoft.com/en-us/windows/apps/design/shell/tiles-and-notifications/send-local-toast-other-apps>
//! - <https://github.com/tauri-apps/winrt-notification/issues/17>（无效 app id 静默失败）
//! - <https://github.com/microsoft/WindowsAppSDK/issues/6821>（只写注册表仍不弹）
//!
//! 之所以不直接改用 PowerShell 的 AUMID 兜底：那条路能弹，但通知来源会写成
//! 「Windows PowerShell」，而且插件的动作按钮 / 点击回调全都不走我们这套了。
//! 宁可花一次启动把身份注册对，也不要长期挂着一个来源不对的降级通道。

use std::path::Path;
use std::sync::Mutex;
use tauri::Manager;

/// 标记文件的内容格式版本。
///
/// **改内容格式就必须改这个值。** `ensure` 的幂等判据是「上次记下的内容 == 这次算出来的」，
/// 而 0.6.2 及更早写进去的是**裸 exe 路径** —— 那时 `IconUri` 指的也是 exe 本身，
/// 于是通知来源那一栏只有名字、没有图标（Windows 的 `IconUri` 只认图片文件）。
/// 只改 `IconUri` 的取值是不够的：老用户的标记照样命中，`ensure` 会直接返回，
/// 升级后仍然没图标。把版本写进标记，等于让每个老安装**重注册一次**。
#[cfg_attr(not(windows), allow(dead_code))]
const MARKER_VERSION: &str = "v2";

/// 标记内容：第一行格式版本，第二行 exe 路径。
///
/// 单独抽出来是为了能被单测钉住 —— 「版本变了，标记就变了」这条约束是这次
/// 修复能不能到达老用户的唯一开关，写在 `ensure` 里就没人测得到了。
///
/// 非 Windows 上只有单测用它（`ensure` 的 Windows 分支才读），所以那边允许
/// dead_code：把测试也 cfg 掉的话，这条断言就只在 Windows CI 上跑，而这台机器
/// （macOS）恰恰是改它的人手边的那一台。
#[cfg_attr(not(windows), allow(dead_code))]
fn marker_text(exe: &Path) -> String {
    format!("{MARKER_VERSION}\n{}", exe.to_string_lossy())
}

/// 最近一次注册结果。`None` = 已就绪（或非 Windows），`Some` = 失败原因。
///
/// **失败不阻断启动**：通知发不出去不该让灯也起不来。状态留给设置页，让人看得到
/// 「为什么没弹」，并提供「重新注册」入口。
#[derive(Default)]
pub struct ToastBackend(Mutex<Option<String>>);

impl ToastBackend {
    pub fn new(error: Option<String>) -> Self {
        Self(Mutex::new(error))
    }

    pub fn error(&self) -> Option<String> {
        self.0.lock().expect("toast backend poisoned").clone()
    }

    pub fn set(&self, error: Option<String>) {
        *self.0.lock().expect("toast backend poisoned") = error;
    }
}

/// 启动时调一次。失败只把原因交回调用方，不 panic、不阻断。
pub fn ensure(identifier: &str) -> Result<(), String> {
    #[cfg(windows)]
    {
        imp::ensure(identifier)
    }
    #[cfg(not(windows))]
    {
        let _ = identifier;
        Ok(())
    }
}

/// 设置页的「重新注册」按钮。返回 `ok` 或一句能直接贴进 issue 的错误。
pub fn retry(app: &tauri::AppHandle) -> String {
    match ensure(&app.config().identifier) {
        Ok(()) => {
            if let Some(backend) = app.try_state::<ToastBackend>() {
                backend.set(None);
            }
            "ok".to_string()
        }
        Err(err) => {
            if let Some(backend) = app.try_state::<ToastBackend>() {
                backend.set(Some(err.clone()));
            }
            err
        }
    }
}

/// 权限徽标：注册失败时不能再说「已允许」。
pub fn failed(app: &tauri::AppHandle) -> bool {
    app.try_state::<ToastBackend>()
        .is_some_and(|backend| backend.error().is_some())
}

/// 诊断报告里的一行。**说清是哪一种「不可用」**，免得「通知没弹」又被归到权限上。
pub fn status_line(app: &tauri::AppHandle) -> String {
    match app
        .try_state::<ToastBackend>()
        .and_then(|backend| backend.error())
    {
        Some(err) => format!("Windows 通知身份：注册失败（{err}）"),
        None if cfg!(windows) => "Windows 通知身份：已注册".to_string(),
        None => "Windows 通知身份：不适用（macOS / Linux 走各自的原生通道）".to_string(),
    }
}

#[cfg(windows)]
mod imp {
    use crate::config;
    use std::path::{Path, PathBuf};
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize, IPersistFile,
    };
    use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PropVariantClear};
    use windows::Win32::UI::Shell::{
        IShellLinkW, SetCurrentProcessExplicitAppUserModelID, SHChangeNotify, SHCNE_ASSOCCHANGED,
        SHCNF_IDLIST, ShellLink,
    };
    use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
    use windows::core::{GUID, Interface, PCWSTR};
    use windows_registry::CURRENT_USER;

    /// 和 `tauri.conf.json` 的 `productName` 一致。通知上显示的名字来自注册表，
    /// 不是快捷方式的文件名 —— 所以两处都得写。
    const PRODUCT_NAME: &str = "SignalHub";
    /// 记着上次给哪个 exe 建过快捷方式。放在 `~/.signalhub/`，不污染开始菜单。
    const MARKER: &str = "toast-shortcut.txt";
    /// 通知来源那一栏的小图标。**必须是图片文件**：`IconUri` 写 exe 时 Windows
    /// 什么都不画（0.6.2 之前的版本就是这么写的，表现正是「通知没有图标」）。
    const ICON_NAME: &str = "toast-icon.png";
    /// 图标从二进制里带出来，不依赖安装包 layout：portable 的 zip 只有一个 exe，
    /// 装完也没有「资源目录」这回事，`include_bytes!` 在哪解压都能拿到同一张图。
    const ICON_PNG: &[u8] = include_bytes!("../icons/128x128.png");

    /// Win32 API 要的是以 0 结尾的 UTF-16。返回一个有所有权的 buffer，调用处
    /// 在用到 `PCWSTR` 的整个表达式里都必须让它活着。
    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// `System.AppUserModel.ID` 的 PROPERTYKEY（Windows SDK `propkey.h`）。
    /// 照抄定义而不是打开 `Win32_Storage_EnhancedStorage` 那个大 feature ——
    /// 为一个常量多编译一整个命名空间不值当。
    fn pkey_app_user_model_id() -> PROPERTYKEY {
        PROPERTYKEY {
            fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
            pid: 5,
        }
    }

    /// Tauri NSIS 默认也把快捷方式放在这里（没配 `startMenuFolder` 时就是
    /// `$SMPROGRAMS\<productName>.lnk`）。用同一个路径，安装版不会被我们多塞一个
    /// 重复的「SignalHub」项；portable 版则正好补上缺的那一枚。
    fn start_menu_shortcut() -> Result<PathBuf, String> {
        let appdata = std::env::var_os("APPDATA")
            .ok_or_else(|| "读不到 %APPDATA%，无法确定开始菜单位置".to_string())?;
        let dir = PathBuf::from(appdata)
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs");
        std::fs::create_dir_all(&dir)
            .map_err(|err| format!("建开始菜单目录失败（{dir:?}）：{err}"))?;
        Ok(dir.join(format!("{PRODUCT_NAME}.lnk")))
    }

    pub(super) fn ensure(identifier: &str) -> Result<(), String> {
        // 1. 进程级 AUMID。它不替代快捷方式，但让窗口分组 / 任务栏和通知用同一个
        //    身份；WindowsAppSDK#6821 里也验证过：只调这一句仍然不弹。
        let id = wide(identifier);
        unsafe { SetCurrentProcessExplicitAppUserModelID(PCWSTR(id.as_ptr())) }
            .map_err(|err| format!("设置进程 AUMID 失败：{err}"))?;

        let exe = std::env::current_exe().map_err(|err| format!("读不到当前 exe 路径：{err}"))?;
        let data = config::data_dir().map_err(|err| format!("读不到数据目录：{err}"))?;
        let marker = data.join(MARKER);
        let shortcut = start_menu_shortcut()?;

        // 2. 图标先落地。**它在幂等判断之前**：判断要确认图标文件还在（用户可能
        //    顺手清了 `~/.signalhub`），而写它本身是幂等的（内容一样就跳过）。
        let icon = data.join(ICON_NAME);
        write_icon(&icon)?;

        // 3. 幂等。**portable 目录会被搬走**，所以判据是「上次记的 exe 还是不是这个」，
        //    不是「快捷方式存在吗」；快捷方式被人删了也要重建。标记里带格式版本，
        //    0.6.2 及更早写的是裸路径，于是**升级后必然重注册一次** —— 那正是把
        //    图标修好的必要一步（老标记命中就会跳过，图标永远补不上）。
        //    写成功才记标记，中途失败下次启动会重试。
        let recorded = std::fs::read_to_string(&marker).unwrap_or_default();
        if recorded == marker_text(&exe)
            && shortcut.exists()
            && icon.exists()
            && registry_ready(identifier)
        {
            return Ok(());
        }

        register_display(identifier, &icon)?;

        // 4. COM 必须在 STA 线程上初始化。单独起一条线程是有意的：Tauri 主线程
        //    可能已经用 MTA 初始化过 COM，那里再 CoInitializeEx(STA) 会拿到
        //    RPC_E_CHANGED_MODE，CoCreateInstance 直接失败。
        let id_owned = identifier.to_string();
        let exe_owned = exe.clone();
        std::thread::Builder::new()
            .name("signalhub-toast-aumid".into())
            .spawn(move || write_shortcut(&id_owned, &exe_owned, &shortcut))
            .map_err(|err| format!("起注册线程失败：{err}"))?
            .join()
            .map_err(|_| "注册通知标识的线程崩了".to_string())??;

        std::fs::write(&marker, marker_text(&exe).as_bytes())
            .map_err(|err| format!("写标记失败（{marker:?}）：{err}"))?;
        Ok(())
    }

    /// 把内嵌的 PNG 写到数据目录。内容一致就不重写 —— 每次启动都写一遍 14KB
    /// 没有意义，而它落在用户目录里，能少一次写就少一次。
    fn write_icon(path: &Path) -> Result<(), String> {
        // 比 `Vec<u8> == &[u8; N]` 更明确：切成切片比，类型一眼能对上。
        // （这条分支只有 Windows 会走，本机编不到，所以宁可写得笨一点。）
        let same = std::fs::read(path)
            .map(|current| current.as_slice() == ICON_PNG.as_slice())
            .unwrap_or(false);
        if same {
            return Ok(());
        }
        std::fs::write(path, ICON_PNG).map_err(|err| format!("写通知图标失败（{path:?}）：{err}"))
    }

    fn registry_ready(identifier: &str) -> bool {
        CURRENT_USER
            .open(format!(r"SOFTWARE\Classes\AppUserModelId\{identifier}"))
            .is_ok()
    }

    /// 通知来源的名字和图标。上游 `tauri-winrt-notification` 的
    /// `examples/unpackaged_app.rs` 就是这三个值；没有它们，Toast 会把
    /// `app.signalhub.desktop` 原样显示给用户。
    ///
    /// **`IconUri` 只能给图片文件**（png / jpg），给 exe 等于没给 —— 通知照常弹，
    /// 左边那一栏只有名字、没有图标。这是 0.6.2 留下的 bug：当时为了让来源显示成
    /// SignalHub 才写这三个值，`IconUri` 顺手填了 exe。微软那篇 unpackaged toast
    /// 文档（2026-04 已下架）与 `Microsoft.Toolkit.Uwp.Notifications` 的做法一致：
    /// 把图标**解出来落到磁盘**，再把那个文件路径写进注册表。
    fn register_display(identifier: &str, icon: &Path) -> Result<(), String> {
        let key = CURRENT_USER
            .create(format!(r"SOFTWARE\Classes\AppUserModelId\{identifier}"))
            .map_err(|err| format!("写 AUMID 注册表键失败：{err}"))?;
        key.set_string("DisplayName", PRODUCT_NAME)
            .map_err(|err| format!("写 DisplayName 失败：{err}"))?;
        key.set_string("IconBackgroundColor", "0")
            .map_err(|err| format!("写 IconBackgroundColor 失败：{err}"))?;
        key.set_string("IconUri", icon.to_string_lossy())
            .map_err(|err| format!("写 IconUri 失败：{err}"))?;
        Ok(())
    }

    fn write_shortcut(identifier: &str, exe: &Path, shortcut: &Path) -> Result<(), String> {
        // SAFETY: 这条线程是我们刚起的，此前没有在这条线程上初始化过 COM。
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if !hr.is_ok() {
            return Err(format!("CoInitializeEx 失败：{hr:?}"));
        }
        let result = create_shortcut(identifier, exe, shortcut);
        // S_OK / S_FALSE 都要配一次 CoUninitialize（后者表示「已初始化过」）。
        unsafe { CoUninitialize() };
        result
    }

    fn create_shortcut(identifier: &str, exe: &Path, shortcut: &Path) -> Result<(), String> {
        let link: IShellLinkW = unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }
            .map_err(|err| format!("CoCreateInstance(ShellLink) 失败：{err}"))?;

        let exe_w = wide(&exe.to_string_lossy());
        unsafe { link.SetPath(PCWSTR(exe_w.as_ptr())) }
            .map_err(|err| format!("设置快捷方式目标失败：{err}"))?;
        if let Some(dir) = exe.parent() {
            let dir_w = wide(&dir.to_string_lossy());
            unsafe { link.SetWorkingDirectory(PCWSTR(dir_w.as_ptr())) }
                .map_err(|err| format!("设置工作目录失败：{err}"))?;
        }
        unsafe { link.SetIconLocation(PCWSTR(exe_w.as_ptr()), 0) }
            .map_err(|err| format!("设置快捷方式图标失败：{err}"))?;
        let desc = wide(PRODUCT_NAME);
        unsafe { link.SetDescription(PCWSTR(desc.as_ptr())) }
            .map_err(|err| format!("设置快捷方式说明失败：{err}"))?;

        // **这一步才是修复本身**：把 AUMID 写进快捷方式的属性存储。
        let store: IPropertyStore = link
            .cast()
            .map_err(|err| format!("拿 IPropertyStore 失败：{err}"))?;
        let key = pkey_app_user_model_id();
        let mut value = PROPVARIANT::from(identifier);
        let set = unsafe { store.SetValue(&key, &value) };
        // From<&str> 内部 SysAllocString 了一个 BSTR；SetValue 会复制，
        // 所以这里必须自己清掉，否则每次注册泄漏一串。
        let _ = unsafe { PropVariantClear(&mut value) };
        set.map_err(|err| format!("写 System.AppUserModel.ID 失败：{err}"))?;
        unsafe { store.Commit() }.map_err(|err| format!("提交快捷方式属性失败：{err}"))?;

        let persist: IPersistFile = link
            .cast()
            .map_err(|err| format!("拿 IPersistFile 失败：{err}"))?;
        let path_w = wide(&shortcut.to_string_lossy());
        unsafe { persist.Save(PCWSTR(path_w.as_ptr()), true) }
            .map_err(|err| format!("保存快捷方式失败（{shortcut:?}）：{err}"))?;

        // 不通知的话 shell 要过一阵才重新扫描开始菜单，表现是「第一次启动
        // 还是不弹、重启一次才好」—— 正好是最难自查的那种假修复。
        unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::ToastBackend;
    use std::path::Path;

    /// 失败原因要能被 UI / 诊断报告原样读出来，且在重试成功后清掉。
    #[test]
    fn backend_state_is_readable_and_clearable() {
        let backend = ToastBackend::new(Some("CoInitializeEx 失败".to_string()));
        assert_eq!(backend.error().as_deref(), Some("CoInitializeEx 失败"));
        backend.set(None);
        assert_eq!(backend.error(), None);
        backend.set(Some("写快捷方式失败".to_string()));
        assert_eq!(backend.error().as_deref(), Some("写快捷方式失败"));
    }

    /// 标记里带着格式版本，而且**和 0.6.2 写的裸路径必然不同**。
    ///
    /// 这是「修好的 `IconUri` 能不能到达老用户」的唯一开关：老安装的
    /// `toast-shortcut.txt` 里是裸 exe 路径，两者一旦相等，`ensure` 就会提前
    /// 返回，注册表里的 `IconUri` 永远停在 exe 上、通知永远没有图标。
    #[test]
    fn the_marker_differs_from_the_legacy_bare_path() {
        let exe = Path::new(r"C:\Apps\SignalHub\signalhub-app.exe");
        let text = super::marker_text(exe);
        assert!(text.starts_with(super::MARKER_VERSION), "{text}");
        assert!(text.contains("signalhub-app.exe"), "{text}");
        assert_ne!(text, exe.to_string_lossy(), "与旧标记相同就不会重注册");
    }

    /// 非 Windows 上 ensure 必须是无副作用的成功，否则 macOS / Linux 的设置页
    /// 会平白多出一条「通知身份注册失败」。
    #[cfg(not(windows))]
    #[test]
    fn ensure_is_a_no_op_off_windows() {
        assert!(super::ensure("app.signalhub.desktop").is_ok());
    }
}
