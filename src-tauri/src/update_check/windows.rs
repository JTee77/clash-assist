//! Windows 更新链（windows-plan r4 §2.4，M5）：NSIS 安装版一键更新。
//!
//! 流程：`download_and_verify`（阶段 1，用户点「一键更新」）→ 前端显示「已下载好」→
//! `launch_installer`（阶段 3，用户点「立即重启」）。两次调用之间，已校验安装包的**只读句柄**
//! （仅允许他人读）一直由本进程持有（[`STAGED`]），任何人都不能写、删、改名；阶段 3 只用这里
//! 保存的路径，不接受前端传入。
//!
//! 句柄纪律（M-4、S-6、S-R5、S-R7、S-R10）：
//! ① 独占写（共享模式 0，`create_new`），经 `tokio::fs::File` 写入，核对字节数 == `asset.size`，上限 64 MiB；
//!   `sync_all` 后 `into_std` 取回 std 句柄再 drop，确定性关闭写句柄；
//! ② 以只读 + `FILE_SHARE_READ` 重新打开（遇到共享冲突——例如 Defender 正在扫描——最多退避重试 3 次），
//!   在这个句柄上用 `spawn_blocking` 算 SHA-256；
//! ③ 读 PE 版本资源 `ProductVersion`，必须等于目标版本；
//! ④ 启动前再确认 semver(target) > 当前版本，写 pending 标记，再 `CreateProcess`；
//!   ② 的句柄保持打开，直到 spawn 返回之后。

use std::ffi::c_void;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, ERROR_SHARING_VIOLATION, ERROR_SUCCESS,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetNamedSecurityInfoW, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateDirectoryW, GetFileAttributesW, GetFileVersionInfoSizeW, GetFileVersionInfoW,
    GetFinalPathNameByHandleW, VerQueryValueW, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_NAME_NORMALIZED, INVALID_FILE_ATTRIBUTES, VOLUME_NAME_DOS,
};
use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};
use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};

use super::install::{self, ChildFate};
use super::pending::{self, PendingMarker};
use super::{
    active_local_proxy_port_async, check, code, fail as coded, is_newer, resolve_download_target,
    sha256_of_reader, stream_asset_into, AssetKind, DownloadLimits, InstallKind, ProgressCallback,
    ReleaseAsset,
};
use crate::mihomo::trust::Who;
use crate::win::sid::OwnedSid;
use crate::win::util::{from_wide, wide, LocalBox};

const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// 安装程序启动后观察多久（M-5）。
const OBSERVE_BUDGET: Duration = Duration::from_secs(2);
const OBSERVE_STEP: Duration = Duration::from_millis(100);
/// ② 遇到共享冲突时的退避（最多重试 3 次）。
const SHARING_BACKOFF: [Duration; 3] = [
    Duration::from_millis(200),
    Duration::from_millis(500),
    Duration::from_millis(1000),
];

/// 已下载、已校验、等待「立即重启」的安装包。
struct Staged {
    dir: PathBuf,
    path: PathBuf,
    /// ② 的只读句柄（共享模式仅读）。持有期间他人不能写、删、改名。
    reader: std::fs::File,
    version: String,
    sha256: String,
}

static STAGED: Mutex<Option<Staged>> = Mutex::new(None);

fn take_staged() -> Option<Staged> {
    STAGED.lock().unwrap_or_else(|e| e.into_inner()).take()
}

fn discard_staged() {
    if let Some(s) = take_staged() {
        drop(s.reader);
        pending::remove_work_dir(&s.dir);
    }
}

// ---- 安装类型（S-10） ------------------------------------------------------------------------

/// 读 `HKCU\<subkey>` 下的 REG_SZ 值（上限 32K 个字符）。
fn read_hkcu_string(subkey: &str, value: &str) -> Option<String> {
    let sk = wide(subkey);
    let vn = wide(value);
    let mut bytes: u32 = 0;
    // SAFETY: 长度查询，数据指针为空。
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            sk.as_ptr(),
            vn.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut bytes,
        )
    };
    if rc != ERROR_SUCCESS || bytes == 0 || bytes > 64 * 1024 {
        return None;
    }
    let mut buf = vec![0u16; (bytes as usize).div_ceil(2) + 1];
    let mut len = (buf.len() * 2) as u32;
    // SAFETY: 缓冲区 `len` 字节，可写。
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            sk.as_ptr(),
            vn.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if rc != ERROR_SUCCESS {
        return None;
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(from_wide(&buf[..n]))
}

/// `GetFinalPathNameByHandleW` 规范化（经 `safe_fs` 打开：拒绝管道 / 设备 / 重解析点本身）。
fn final_path(p: &Path) -> Option<String> {
    let f = crate::safe_fs::open_attrs(p).ok()?;
    let mut buf = vec![0u16; 1024];
    loop {
        // SAFETY: `f` 持有有效句柄；缓冲区长度如实传入。
        let n = unsafe {
            GetFinalPathNameByHandleW(
                f.as_raw_handle(),
                buf.as_mut_ptr(),
                buf.len() as u32,
                FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
            )
        } as usize;
        if n == 0 {
            return None;
        }
        if n < buf.len() {
            return Some(from_wide(&buf[..n]));
        }
        if n > 32 * 1024 {
            return None;
        }
        buf = vec![0u16; n + 1];
    }
}

pub fn install_kind() -> InstallKind {
    let loc = read_hkcu_string(install::UNINSTALL_SUBKEY, "InstallLocation");
    let bin = read_hkcu_string(install::UNINSTALL_SUBKEY, "MainBinaryName");
    let exe = std::env::current_exe().ok().and_then(|p| final_path(&p));
    install::classify_install(
        loc.as_deref(),
        bin.as_deref(),
        exe.as_deref(),
        |p| final_path(Path::new(p)),
        // 与 classify_install 内剥前缀双保险：safe_fs 只接受 X:\…（#61 e2e）
        |p| crate::safe_fs::is_file(Path::new(install::strip_win32_verbatim_prefix(p))),
    )
}

// ---- 私有更新目录（M-4） ----------------------------------------------------------------------

fn random_hex(bytes: usize) -> Result<String, String> {
    let mut b = vec![0u8; bytes];
    getrandom::fill(&mut b).map_err(|e| format!("生成随机数失败：{e}"))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// 路径必须是已存在的目录且不是重解析点（`GetFileAttributesW` 不打开文件）。
fn check_plain_dir(p: &Path) -> Result<(), String> {
    let w = wide(&p.to_string_lossy());
    // SAFETY: 以 NUL 结尾的宽字符串。
    let attrs = unsafe { GetFileAttributesW(w.as_ptr()) };
    if attrs == INVALID_FILE_ATTRIBUTES {
        return Err(format!("无法读取目录属性：{}", p.display()));
    }
    if attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(format!(
            "拒绝重解析点（联接点 / 符号链接）：{}",
            p.display()
        ));
    }
    if attrs & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(format!("不是目录：{}", p.display()));
    }
    Ok(())
}

/// 目录属主必须是本人（或 SYSTEM / BUILTIN\Administrators：以管理员身份运行过时属主是管理员组）。
fn check_owner(p: &Path) -> Result<(), String> {
    let known = crate::win::identity::known().map_err(|e| format!("读取本人身份失败：{e:?}"))?;
    let w = wide(&p.to_string_lossy());
    let mut owner: PSID = std::ptr::null_mut();
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: 输出指针有效；安全描述符经 LocalBox 释放，owner 指向其内部。
    let rc = unsafe {
        GetNamedSecurityInfoW(
            w.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut sd,
        )
    };
    if rc != ERROR_SUCCESS {
        return Err(format!("读取目录属主失败（{rc}）：{}", p.display()));
    }
    // SAFETY: 文档要求 LocalFree。
    let _sd = unsafe { LocalBox::from_raw(sd) };
    // SAFETY: owner 指向 `_sd` 内的有效 SID。
    let sid = unsafe { OwnedSid::copy_from(owner, "GetNamedSecurityInfoW") }
        .map_err(|e| format!("读取目录属主失败：{e:?}"))?;
    match known.classify(&sid).who {
        Who::Me | Who::System | Who::Admins => Ok(()),
        Who::Other => Err(format!("目录属主不是本人：{}", p.display())),
    }
}

/// 不存在则创建（已存在不算错）；随后检查不是重解析点、属主可信。
fn ensure_trusted_dir(p: &Path) -> Result<(), String> {
    crate::safe_fs::check_path_shape(p).map_err(|e| e.to_string())?;
    match std::fs::create_dir(p) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("创建目录失败：{e}：{}", p.display())),
    }
    check_plain_dir(p)?;
    check_owner(p)
}

/// 以 SDDL `D:P(A;OICI;FA;;;<本人SID>)(A;OICI;FA;;;SY)` **新建**目录；已存在即失败。
fn create_private_dir(p: &Path) -> Result<(), String> {
    crate::safe_fs::check_path_shape(p).map_err(|e| e.to_string())?;
    let known = crate::win::identity::known().map_err(|e| format!("读取本人身份失败：{e:?}"))?;
    let sddl = wide(&format!(
        "D:P(A;OICI;FA;;;{})(A;OICI;FA;;;SY)",
        known.me_str
    ));
    let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: 以 NUL 结尾的 SDDL；输出经 LocalBox 释放。
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1, // SDDL_REVISION_1
            &mut sd,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(format!(
            "构造安全描述符失败：{}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: 文档要求 LocalFree。
    let sd = unsafe { LocalBox::from_raw(sd) };
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.as_ptr(),
        bInheritHandle: 0,
    };
    let w = wide(&p.to_string_lossy());
    // SAFETY: 路径以 NUL 结尾；`sa` 与 `sd` 在调用期间存活。
    if unsafe { CreateDirectoryW(w.as_ptr(), &sa) } == 0 {
        let e = std::io::Error::last_os_error();
        return Err(if e.raw_os_error() == Some(ERROR_ALREADY_EXISTS as i32) {
            format!("更新目录已存在，拒绝复用：{}", p.display())
        } else {
            format!("创建更新目录失败：{e}")
        });
    }
    check_plain_dir(p)
}

/// `%LOCALAPPDATA%\ClashAssist\updates\<32 位随机十六进制>\`（M-4）：父目录不存在时创建，
/// 检查属主与重解析点；本次工作目录以私有 DACL 新建，已存在即失败（不用 `create_dir_all`）。
pub(crate) fn prepare_update_dir(root: &Path) -> Result<PathBuf, String> {
    ensure_trusted_dir(root)?;
    let updates = pending::updates_root(root);
    ensure_trusted_dir(&updates)?;
    let dir = updates.join(random_hex(16)?);
    create_private_dir(&dir)?;
    Ok(dir)
}

// ---- 下载与校验（M-4、S-6） -------------------------------------------------------------------

fn is_sharing_violation(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(ERROR_SHARING_VIOLATION as i32)
}

/// ② 只读 + 仅允许他人读；共享冲突时按 [`SHARING_BACKOFF`] 重试。
fn open_read_shared_with_retry(path: &Path) -> Result<std::fs::File, String> {
    let mut backoff = SHARING_BACKOFF.iter();
    loop {
        match crate::safe_fs::open_read_share_read(path) {
            Ok(f) => return Ok(f),
            Err(e) if is_sharing_violation(&e) => match backoff.next() {
                Some(d) => std::thread::sleep(*d),
                None => return Err(format!("安装包被占用，无法校验：{e}")),
            },
            Err(e) => return Err(format!("打开安装包失败：{e}")),
        }
    }
}

/// ①②：下载到 `path`（独占创建），核对大小，关闭写句柄，再以只读共享句柄算 SHA-256 并比对。
/// 成功时返回该只读句柄（调用方一直持有到安装程序启动之后）。
pub(crate) async fn fetch_and_verify(
    url: &str,
    path: &Path,
    expected_size: u64,
    expected_sha256: &str,
    proxy_port: Option<u16>,
    on_progress: ProgressCallback<'_>,
) -> Result<std::fs::File, String> {
    let open_path = path.to_path_buf();
    let w = stream_asset_into(
        url,
        CURRENT,
        proxy_port,
        on_progress,
        DownloadLimits {
            max_bytes: AssetKind::WinSetupX64.max_bytes(),
            expected_size: Some(expected_size),
        },
        move || {
            crate::safe_fs::create_new_exclusive(&open_path)
                .map(tokio::fs::File::from_std)
                .map_err(|e| format!("创建安装包文件失败：{e}"))
        },
    )
    .await?;
    w.sync_all().await.map_err(|e| format!("落盘失败：{e}"))?;
    let w = w.into_std().await;
    drop(w); // S-R10：确定性关闭写句柄，避免 ② 遇到共享冲突

    let read_path = path.to_path_buf();
    let expected = expected_sha256.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let mut r = open_read_shared_with_retry(&read_path)?;
        let digest = sha256_of_reader(&mut r).map_err(|e| format!("读取安装包失败：{e}"))?;
        if digest != expected {
            return Err(coded(
                code::DIGEST_MISMATCH,
                format!("安装包 SHA-256 校验不匹配（期望 {expected}，实际 {digest}）"),
            ));
        }
        Ok(r)
    })
    .await
    .map_err(|e| format!("校验任务异常终止：{e}"))?
}

/// ③ PE 版本资源里的 `ProductVersion`（`GetFileVersionInfoSizeW` / `GetFileVersionInfoW` / `VerQueryValueW`）。
pub(crate) fn product_version(path: &Path) -> Option<String> {
    let w = wide(&path.to_string_lossy());
    let mut handle = 0u32;
    // SAFETY: 以 NUL 结尾的路径。
    let size = unsafe { GetFileVersionInfoSizeW(w.as_ptr(), &mut handle) };
    if size == 0 || size > 16 * 1024 * 1024 {
        return None;
    }
    let mut block = vec![0u8; size as usize];
    // SAFETY: 缓冲区 `size` 字节。
    if unsafe { GetFileVersionInfoW(w.as_ptr(), 0, size, block.as_mut_ptr().cast()) } == 0 {
        return None;
    }
    let query = |sub: &str| -> Option<(*const c_void, u32)> {
        let s = wide(sub);
        let mut p: *mut c_void = std::ptr::null_mut();
        let mut len = 0u32;
        // SAFETY: `block` 是 GetFileVersionInfoW 填充的版本块；输出指针指向 `block` 内部。
        let ok = unsafe { VerQueryValueW(block.as_ptr().cast(), s.as_ptr(), &mut p, &mut len) };
        (ok != 0 && !p.is_null()).then_some((p as *const c_void, len))
    };
    let mut langs: Vec<(u16, u16)> = Vec::new();
    if let Some((p, len)) = query(r"\VarFileInfo\Translation") {
        let n = (len as usize) / 4;
        // SAFETY: Translation 是 n 个 (u16 语言, u16 代码页) 对，位于 `block` 内。
        let pairs = unsafe { std::slice::from_raw_parts(p as *const u16, n * 2) };
        for [lang, cp] in pairs.as_chunks::<2>().0 {
            langs.push((*lang, *cp));
        }
    }
    for fallback in [(0x0409, 0x04b0), (0x0409, 0x04e4), (0x0000, 0x04b0)] {
        if !langs.contains(&fallback) {
            langs.push(fallback);
        }
    }
    for (lang, cp) in langs {
        let key = format!(r"\StringFileInfo\{lang:04x}{cp:04x}\ProductVersion");
        if let Some((p, len)) = query(&key) {
            if len == 0 {
                continue;
            }
            // SAFETY: 字符串值为 `len` 个 UTF-16 字符（含结尾 NUL），位于 `block` 内。
            let s = unsafe { std::slice::from_raw_parts(p as *const u16, len as usize) };
            let n = s.iter().position(|&c| c == 0).unwrap_or(s.len());
            return Some(from_wide(&s[..n]));
        }
    }
    None
}

/// 阶段 1：下载最新安装包到私有更新目录并完成全部校验（①②③），保存只读句柄，返回工作目录。
pub async fn download_and_verify(on_progress: ProgressCallback<'_>) -> Result<String, String> {
    if install_kind() != InstallKind::Nsis {
        return Err(coded(
            code::PORTABLE,
            "便携版不支持一键更新，请前往发布页下载",
        ));
    }
    let info = check(CURRENT).await?;
    let asset: ReleaseAsset = resolve_download_target(&info)?;
    if asset.kind != AssetKind::WinSetupX64 {
        return Err(coded(
            code::ASSET_MISSING,
            "最新 Release 中没有 Windows 安装包",
        ));
    }
    // 上一次下载但没有启动的安装包：丢弃（重试时重新下载并校验，不复用旧文件）。
    discard_staged();
    let root = crate::platform::app_data_dir().ok_or("无法定位 %LOCALAPPDATA%")?;
    let dir = tauri::async_runtime::spawn_blocking(move || prepare_update_dir(&root))
        .await
        .map_err(|e| format!("准备更新目录异常终止：{e}"))??;
    let path = dir.join(format!("setup-{}.exe", random_hex(8)?));

    let result = async {
        let proxy = active_local_proxy_port_async().await;
        let reader = fetch_and_verify(
            &asset.url,
            &path,
            asset.size,
            &asset.sha256,
            proxy,
            on_progress,
        )
        .await?;
        let pv_path = path.clone();
        let found = tauri::async_runtime::spawn_blocking(move || product_version(&pv_path))
            .await
            .map_err(|e| format!("读取版本资源异常终止：{e}"))?;
        install::check_product_version(found.as_deref(), &asset.version)
            .map_err(|e| coded(code::PE_VERSION_MISMATCH, e))?;
        Ok::<_, String>(reader)
    }
    .await;

    match result {
        Ok(reader) => {
            crate::append_app_log(&format!(
                "[update] 已下载并校验 {}（{} 字节）",
                asset.name, asset.size
            ));
            *STAGED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Staged {
                dir: dir.clone(),
                path,
                reader,
                version: asset.version,
                sha256: asset.sha256,
            });
            Ok(dir.to_string_lossy().to_string())
        }
        Err(e) => {
            pending::remove_work_dir(&dir);
            Err(format!("下载更新包失败：{e}"))
        }
    }
}

// ---- 启动安装程序（M-5、S-7） -----------------------------------------------------------------

fn launch_blocking(staged: Staged, root: &Path) -> Result<ChildFate, String> {
    let fail = |staged_dir: &Path, msg: String| -> Result<ChildFate, String> {
        pending::clear(root, None);
        pending::remove_work_dir(staged_dir);
        Err(msg)
    };
    // ④ 启动前再做一次 semver 比较（M-5）
    if !is_newer(&staged.version, CURRENT) {
        let dir = staged.dir.clone();
        drop(staged);
        return fail(
            &dir,
            coded(
                code::NOT_NEWER,
                format!("拒绝安装：目标版本不比当前版本 {CURRENT} 新"),
            ),
        );
    }
    let marker = PendingMarker {
        from: CURRENT.to_string(),
        to: staged.version.clone(),
        sha256: staged.sha256.clone(),
        installer_path: staged.path.to_string_lossy().to_string(),
        started_at: pending::now_unix(),
    };
    if let Err(e) = pending::write(root, &marker) {
        let dir = staged.dir.clone();
        drop(staged);
        return fail(&dir, e);
    }
    // 全部校验（下载时的 size / digest / PE 版本，启动前的 semver 复核）都已通过，紧接着就是 CreateProcess。
    // 这一行只证明「尝试启动」，e2e 用它与 4688 的映像路径逐字节比较（docs/windows-updater.md「成功路径的证据」）。
    crate::append_app_log(&install::launch_attempt_line(
        std::process::id(),
        &staged.version,
        &staged.path.to_string_lossy(),
    ));
    // S-7：CreateProcess，不经 ShellExecute（不走外壳的 SmartScreen 检查路径）。
    let spawned = std::process::Command::new(&staged.path)
        .args(["/S", "/UPDATE", "/R"])
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn();
    // S-R5：只读句柄保持到 spawn 返回之后（CreateProcess 返回前已完成映像映射）。
    let Staged { dir, reader, .. } = staged;
    drop(reader);
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            return fail(
                &dir,
                coded(
                    code::INSTALLER_SPAWN_FAILED,
                    format!("启动安装程序失败：{e}"),
                ),
            )
        }
    };
    let fate = install::observe_child(
        || child.try_wait().map(|s| s.map(|s| s.code())),
        OBSERVE_BUDGET,
        OBSERVE_STEP,
        std::thread::sleep,
    );
    crate::append_app_log(&format!("[update] 安装程序：{fate:?}"));
    match fate {
        ChildFate::ExitedErr(exit) => fail(
            &dir,
            coded(
                code::INSTALLER_EXIT_NONZERO,
                format!(
                    "安装程序退出码 {}（可能是降级被拒绝或被系统拦截），未完成更新",
                    exit.map(|c| c.to_string()).unwrap_or_else(|| "未知".into())
                ),
            ),
        ),
        ok => {
            crate::append_app_log(&format!(
                "[update] installer launched to={} fate={ok:?}，本进程退出",
                marker.to
            ));
            Ok(ok)
        }
    }
}

/// 阶段 3：启动已校验的安装程序（`/S /UPDATE /R`）。成功（观察期内以 0 退出，或 2 s 后仍在运行）
/// 时本进程退出；失败时清理目录与标记、不退出，由前端提示并给出「前往发布页」。
pub async fn launch_installer(app: tauri::AppHandle) -> Result<(), String> {
    let staged = take_staged().ok_or_else(|| {
        coded(
            code::NO_STAGED,
            "没有已下载并校验的更新包，请重新点击「一键更新」",
        )
    })?;
    let root = crate::platform::app_data_dir().ok_or("无法定位 %LOCALAPPDATA%")?;
    tauri::async_runtime::spawn_blocking(move || launch_blocking(staged, &root))
        .await
        .map_err(|e| format!("启动安装程序异常终止：{e}"))??;
    app.exit(0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("ca-m5-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn prepare_update_dir_creates_fresh_private_dirs() {
        let root = temp_root("prep");
        let a = prepare_update_dir(&root).unwrap();
        let b = prepare_update_dir(&root).unwrap();
        assert_ne!(a, b);
        for d in [&a, &b] {
            let name = d.file_name().unwrap().to_str().unwrap();
            assert!(pending::is_work_dir_name(name), "{name}");
            assert_eq!(d.parent().unwrap(), pending::updates_root(&root));
            assert!(crate::safe_fs::is_dir(d));
        }
        // 已存在即失败（不复用）
        let err = create_private_dir(&a).unwrap_err();
        assert!(err.contains("已存在"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn prepare_update_dir_rejects_junction_parent() {
        let root = temp_root("junction");
        std::fs::create_dir_all(&root).unwrap();
        let target = temp_root("junction-target");
        std::fs::create_dir_all(&target).unwrap();
        let link = pending::updates_root(&root);
        let st = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .unwrap();
        assert!(st.status.success(), "mklink /J 失败：{st:?}");
        let err = prepare_update_dir(&root).unwrap_err();
        assert!(err.contains("重解析点"), "{err}");
        // 联接点后面的目录里什么都没建
        assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);
        let _ = std::fs::remove_dir(&link);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&target);
    }

    /// 测试进程不是从安装目录运行的：一律判为便携版（不会误开一键更新）。
    #[test]
    fn test_binary_is_portable() {
        assert_eq!(install_kind(), InstallKind::Portable);
        let exe = std::env::current_exe().unwrap();
        let fp = final_path(&exe).expect("当前 exe 应能规范化");
        assert!(install::normalize_win_path(&fp).ends_with(".exe"), "{fp}");
    }

    #[test]
    fn product_version_reads_system_binary() {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        let notepad = Path::new(&windir).join(r"System32\notepad.exe");
        let v = product_version(&notepad).expect("notepad.exe 应有 ProductVersion");
        assert!(v.starts_with("10.") || v.starts_with("6."), "{v}");
        // 非 PE 文件没有版本资源
        let root = temp_root("pv");
        std::fs::create_dir_all(&root).unwrap();
        let f = root.join("x.exe");
        crate::safe_fs::write_for_test(&f, b"not a pe").unwrap();
        assert_eq!(product_version(&f), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ①②：独占写句柄关闭后，只读共享句柄存在期间任何人都不能再以写方式打开。
    #[test]
    fn verified_reader_blocks_writers() {
        let root = temp_root("share");
        std::fs::create_dir_all(&root).unwrap();
        let p = root.join("setup.exe");
        {
            use std::io::Write;
            let mut w = crate::safe_fs::create_new_exclusive(&p).unwrap();
            // 独占写期间别人连读都打不开
            let e = crate::safe_fs::open_read_share_read(&p).unwrap_err();
            assert!(is_sharing_violation(&e), "{e:?}");
            w.write_all(b"abc").unwrap();
        }
        // 已存在：create_new 失败
        assert!(crate::safe_fs::create_new_exclusive(&p).is_err());
        let mut r = open_read_shared_with_retry(&p).unwrap();
        assert_eq!(
            sha256_of_reader(&mut r).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // 持有只读句柄时：可以再读，不能写、不能删
        assert!(crate::safe_fs::open_read_share_read(&p).is_ok());
        let e = crate::safe_fs::create_truncate(&p).unwrap_err();
        assert!(is_sharing_violation(&e), "{e:?}");
        assert!(std::fs::remove_file(&p).is_err());
        drop(r);
        assert!(std::fs::remove_file(&p).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    fn serve_once(body: Vec<u8>, content_length: usize) -> (u16, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            if let Ok((mut s, _)) = listener.accept() {
                let _ = s.set_read_timeout(Some(Duration::from_millis(1000)));
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {content_length}\r\nConnection: close\r\n\r\n"
                );
                let _ = s.write_all(head.as_bytes());
                let _ = s.write_all(&body);
            }
        });
        (port, h)
    }

    #[test]
    fn fetch_and_verify_checks_size_and_digest() {
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let root = temp_root("fetch");
        std::fs::create_dir_all(&root).unwrap();
        let run = |name: &str, size: u64, sha: &str, body: &[u8], cl: usize| {
            let (port, h) = serve_once(body.to_vec(), cl);
            let url = format!("http://127.0.0.1:{port}/s.exe");
            let p = root.join(name);
            let r = tauri::async_runtime::block_on(fetch_and_verify(
                &url,
                &p,
                size,
                sha,
                None,
                &|_| {},
            ));
            let _ = h.join();
            (r, p)
        };
        let (ok, p) = run("ok.exe", 3, abc, b"abc", 3);
        let reader = ok.expect("正确的大小与摘要应通过");
        // 句柄持有期间文件不能被改写
        assert!(crate::safe_fs::create_truncate(&p).is_err());
        drop(reader);
        // 大小与 API 记录不符（Content-Length 就不对）
        let (r, _) = run("size.exe", 4, abc, b"abc", 3);
        assert!(r.unwrap_err().contains("大小"));
        // 摘要不符
        let (r, _) = run("sha.exe", 3, &"0".repeat(64), b"abc", 3);
        assert!(r.unwrap_err().contains("SHA-256"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn spawned_child_nonzero_exit_is_observed() {
        let mut child = std::process::Command::new("cmd")
            .args(["/C", "exit 3"])
            .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
            .spawn()
            .unwrap();
        let fate = install::observe_child(
            || child.try_wait().map(|s| s.map(|s| s.code())),
            OBSERVE_BUDGET,
            OBSERVE_STEP,
            std::thread::sleep,
        );
        assert_eq!(fate, ChildFate::ExitedErr(Some(3)));
    }

    #[test]
    fn launch_refuses_non_newer_and_cleans_up() {
        let root = temp_root("launch");
        let dir = prepare_update_dir(&root).unwrap();
        let path = dir.join("setup-0000000000000000.exe");
        crate::safe_fs::write_for_test(&path, b"MZ").unwrap();
        let reader = crate::safe_fs::open_read_share_read(&path).unwrap();
        let staged = Staged {
            dir: dir.clone(),
            path,
            reader,
            version: CURRENT.to_string(),
            sha256: "00".repeat(32),
        };
        let err = launch_blocking(staged, &root).unwrap_err();
        assert!(err.contains("不比当前版本"), "{err}");
        assert!(!crate::safe_fs::exists(&dir));
        assert!(!crate::safe_fs::exists(&pending::marker_path(&root)));
        let _ = std::fs::remove_dir_all(&root);
    }
}
