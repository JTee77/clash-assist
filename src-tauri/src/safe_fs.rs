//! 配置 / 日志文件的安全打开封装（windows-plan r4 §2.1「强制手段（M-2）」、S-R1）。
//!
//! 背景（V3）：Windows 上按路径打开 / 探测的 std API 走 CreateFileW 且不带 SQOS；路径若是
//! 命名管道（或最终指向管道的重解析点），服务端就能以 SecurityImpersonation 级别模拟本进程。
//! 这里是 clippy `disallowed-methods` 之外**唯一**允许按路径打开文件的地方：
//!
//! - Windows：只接受盘符绝对路径（`X:\…`），拒绝 `\\.\`、`\\?\`、UNC 等设备 / 网络前缀；
//!   打开时带 `SECURITY_SQOS_PRESENT | SECURITY_ANONYMOUS` 与 `FILE_FLAG_OPEN_REPARSE_POINT`
//!   （不跟随最后一级重解析点），打开后再校验 `GetFileType == FILE_TYPE_DISK` 且不是重解析点；
//!   读取有上限。
//! - macOS / Linux：直接调用 std（不在 S-R1 的 Windows 风险面），行为与 M0 之前一致。
//!
//! 命名管道只能经 `mihomo::pipe::open_verified`（类型约束见 `guard::PipeName`）。

use std::io::{self, Read};
use std::path::Path;

/// 读取上限：Verge 的 config.yaml 可能内嵌整份订阅，留足余量。
pub const MAX_CONFIG_BYTES: u64 = 32 * 1024 * 1024;

// ---- Windows -------------------------------------------------------------------------------

#[cfg(windows)]
mod imp {
    use std::fs::{File, OpenOptions};
    use std::io;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;

    use windows_sys::Win32::Storage::FileSystem::{
        GetFileType, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_TYPE_DISK, SECURITY_ANONYMOUS,
    };

    /// 只接受 `X:\…` 形式的绝对路径。
    pub fn check_path_shape(path: &Path) -> io::Result<()> {
        let s = path.to_string_lossy();
        let b = s.as_bytes();
        let drive_abs = b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && (b[2] == b'\\' || b[2] == b'/');
        if !drive_abs || s.contains('\0') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("拒绝非盘符绝对路径（防止打开命名管道 / 设备，S-R1）：{s}"),
            ));
        }
        Ok(())
    }

    fn check_handle(f: &File, path: &Path) -> io::Result<()> {
        // SAFETY: `f` 持有有效句柄，调用期间存活。
        let ty = unsafe { GetFileType(f.as_raw_handle()) };
        if ty != FILE_TYPE_DISK {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("不是磁盘文件（GetFileType={ty}）：{}", path.display()),
            ));
        }
        let md = f.metadata()?;
        if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("拒绝重解析点：{}", path.display()),
            ));
        }
        Ok(())
    }

    fn open_with(opts: &mut OpenOptions, path: &Path) -> io::Result<File> {
        check_path_shape(path)?;
        opts.security_qos_flags(SECURITY_ANONYMOUS)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS);
        #[allow(
            clippy::disallowed_methods,
            reason = "安全打开封装（safe_fs）：盘符路径 + SECURITY_ANONYMOUS + OPEN_REPARSE_POINT，打开后校验 FILE_TYPE_DISK"
        )]
        let f = opts.open(path)?;
        check_handle(&f, path)?;
        Ok(f)
    }

    pub fn open_read(path: &Path) -> io::Result<File> {
        open_with(OpenOptions::new().read(true), path)
    }

    pub fn open_attrs(path: &Path) -> io::Result<File> {
        open_with(OpenOptions::new().access_mode(FILE_READ_ATTRIBUTES), path)
    }

    pub fn open_append(path: &Path) -> io::Result<File> {
        open_with(OpenOptions::new().create(true).append(true), path)
    }

    #[cfg(test)]
    pub fn create_truncate(path: &Path) -> io::Result<File> {
        open_with(
            OpenOptions::new().write(true).create(true).truncate(true),
            path,
        )
    }

    pub fn exists(path: &Path) -> bool {
        open_attrs(path).is_ok()
    }

    pub fn is_dir(path: &Path) -> bool {
        open_attrs(path)
            .and_then(|f| f.metadata())
            .map(|m| m.is_dir())
            .unwrap_or(false)
    }

    pub fn is_file(path: &Path) -> bool {
        open_attrs(path)
            .and_then(|f| f.metadata())
            .map(|m| m.is_file())
            .unwrap_or(false)
    }
}

// ---- macOS / Linux -------------------------------------------------------------------------

#[cfg(not(windows))]
mod imp {
    #[cfg(test)]
    #[allow(
        clippy::disallowed_methods,
        reason = "仅非 Windows 编译，不在 S-R1 的 Windows 风险面"
    )]
    pub fn create_truncate(path: &Path) -> io::Result<File> {
        File::create(path)
    }

    use std::fs::{File, OpenOptions};
    use std::io;
    use std::path::Path;

    #[allow(
        clippy::disallowed_methods,
        reason = "仅非 Windows 编译，不在 S-R1 的 Windows 风险面"
    )]
    pub fn open_read(path: &Path) -> io::Result<File> {
        File::open(path)
    }

    #[allow(
        clippy::disallowed_methods,
        reason = "仅非 Windows 编译，不在 S-R1 的 Windows 风险面"
    )]
    pub fn open_append(path: &Path) -> io::Result<File> {
        OpenOptions::new().create(true).append(true).open(path)
    }

    #[allow(
        clippy::disallowed_methods,
        reason = "仅非 Windows 编译，不在 S-R1 的 Windows 风险面"
    )]
    pub fn exists(path: &Path) -> bool {
        path.exists()
    }

    #[allow(
        clippy::disallowed_methods,
        reason = "仅非 Windows 编译，不在 S-R1 的 Windows 风险面"
    )]
    pub fn is_dir(path: &Path) -> bool {
        path.is_dir()
    }

    #[allow(
        clippy::disallowed_methods,
        reason = "仅非 Windows 编译，不在 S-R1 的 Windows 风险面"
    )]
    pub fn is_file(path: &Path) -> bool {
        path.is_file()
    }
}

/// 读取 UTF-8 文本文件（上限 [`MAX_CONFIG_BYTES`]）。
pub fn read_to_string(path: &Path) -> io::Result<String> {
    let f = imp::open_read(path)?;
    let mut buf = Vec::new();
    f.take(MAX_CONFIG_BYTES + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > MAX_CONFIG_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("文件超过 {MAX_CONFIG_BYTES} 字节：{}", path.display()),
        ));
    }
    String::from_utf8(buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// 路径存在（Windows：且是磁盘上的普通文件或目录，不是重解析点）。
pub fn exists(path: &Path) -> bool {
    imp::exists(path)
}

/// 路径是目录。
pub fn is_dir(path: &Path) -> bool {
    imp::is_dir(path)
}

/// 路径是普通文件。
pub fn is_file(path: &Path) -> bool {
    imp::is_file(path)
}

/// 列目录。Windows 上先做路径形状检查（拒绝 `\\.\pipe\` 等设备前缀）；`read_dir` 本身
/// 走 FindFirstFileW，不会打开目录项。
pub fn read_dir(path: &Path) -> io::Result<std::fs::ReadDir> {
    #[cfg(windows)]
    imp::check_path_shape(path)?;
    std::fs::read_dir(path)
}

/// 以追加方式打开（不存在则创建），用于应用日志。
pub fn open_append(path: &Path) -> io::Result<std::fs::File> {
    imp::open_append(path)
}

/// 测试辅助：创建 / 截断并写入（同样走安全打开封装）。
#[cfg(test)]
pub fn write_for_test(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    imp::create_truncate(path)?.write_all(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("ca-safe-fs-{}-{name}", std::process::id()))
    }

    #[test]
    fn read_exists_is_dir_append_roundtrip() {
        let dir = tmp("dir");
        std::fs::create_dir_all(&dir).unwrap();
        assert!(exists(&dir) && is_dir(&dir));
        let f = dir.join("a.yaml");
        {
            use std::io::Write;
            let mut h = open_append(&f).unwrap();
            writeln!(h, "secret: x").unwrap();
        }
        assert!(exists(&f) && !is_dir(&f));
        assert_eq!(read_to_string(&f).unwrap(), "secret: x\n");
        let missing = dir.join("missing.yaml");
        assert!(!exists(&missing) && !is_dir(&missing));
        assert!(read_to_string(&missing).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 管道 / 设备 / UNC 路径在打开之前就被拒绝（不会发出 CreateFileW）。
    #[cfg(windows)]
    #[test]
    fn windows_rejects_device_and_pipe_paths_before_open() {
        for p in [
            r"\\.\pipe\verge-mihomo",
            r"\\?\C:\x.yaml",
            r"\\server\share\x.yaml",
            r"//./pipe/verge-mihomo",
            r"relative\x.yaml",
            r"C:",
        ] {
            let p = Path::new(p);
            assert!(imp::check_path_shape(p).is_err(), "{p:?}");
            assert!(
                !exists(p) && !is_dir(p) && read_to_string(p).is_err(),
                "{p:?}"
            );
        }
    }

    /// 打开后的类型校验：NUL 设备不是磁盘文件。
    #[cfg(windows)]
    #[test]
    fn windows_rejects_non_disk_handles() {
        // `C:\NUL` 解析为 NUL 设备（FILE_TYPE_CHAR）。
        assert!(read_to_string(Path::new(r"C:\NUL")).is_err());
    }
}
