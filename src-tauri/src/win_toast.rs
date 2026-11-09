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
//! 上游依据：
//! - <https://learn.microsoft.com/en-us/windows/apps/design/shell/tiles-and-notifications/send-local-toast-other-apps>
//! - <https://github.com/tauri-apps/winrt-notification/issues/17>（无效 app id 静默失败）
//! - <https://github.com/microsoft/WindowsAppSDK/issues/6821>（只写注册表仍不弹）
//!
//! 之所以不直接改用 PowerShell 的 AUMID 兜底：那条路能弹，但通知来源会写成
//! 「Windows PowerShell」，而且插件的动作按钮 / 点击回调全都不走我们这套了。
//! 宁可花一次启动把身份注册对，也不要长期挂着一个来源不对的降级通道。

use std::sync::Mutex;
use tauri::Manager;

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
        let marker = config::data_dir()
            .map_err(|err| format!("读不到数据目录：{err}"))?
            .join(MARKER);
        let shortcut = start_menu_shortcut()?;

        // 2. 幂等。**portable 目录会被搬走**，所以判据是「上次记的 exe 还是不是这个」，
        //    不是「快捷方式存在吗」；快捷方式被人删了也要重建。写成功才记标记，
        //    中途失败下次启动会重试。
        let recorded = std::fs::read_to_string(&marker).unwrap_or_default();
        if recorded == exe.to_string_lossy() && shortcut.exists() && registry_ready(identifier) {
            return Ok(());
        }

        register_display(identifier, &exe)?;

        // 3. COM 必须在 STA 线程上初始化。单独起一条线程是有意的：Tauri 主线程
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

        std::fs::write(&marker, exe.to_string_lossy().as_bytes())
            .map_err(|err| format!("写标记失败（{marker:?}）：{err}"))?;
        Ok(())
    }

    fn registry_ready(identifier: &str) -> bool {
        CURRENT_USER
            .open(format!(r"SOFTWARE\Classes\AppUserModelId\{identifier}"))
            .is_ok()
    }

    /// 通知来源的名字和图标。上游 `tauri-winrt-notification` 的
    /// `examples/unpackaged_app.rs` 就是这三个值；没有它们，Toast 会把
    /// `app.signalhub.desktop` 原样显示给用户。
    fn register_display(identifier: &str, exe: &Path) -> Result<(), String> {
        let key = CURRENT_USER
            .create(format!(r"SOFTWARE\Classes\AppUserModelId\{identifier}"))
            .map_err(|err| format!("写 AUMID 注册表键失败：{err}"))?;
        key.set_string("DisplayName", PRODUCT_NAME)
            .map_err(|err| format!("写 DisplayName 失败：{err}"))?;
        key.set_string("IconBackgroundColor", "0")
            .map_err(|err| format!("写 IconBackgroundColor 失败：{err}"))?;
        key.set_string("IconUri", exe.to_string_lossy())
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

    /// 非 Windows 上 ensure 必须是无副作用的成功，否则 macOS / Linux 的设置页
    /// 会平白多出一条「通知身份注册失败」。
    #[cfg(not(windows))]
    #[test]
    fn ensure_is_a_no_op_off_windows() {
        assert!(super::ensure("app.signalhub.desktop").is_ok());
    }
}
