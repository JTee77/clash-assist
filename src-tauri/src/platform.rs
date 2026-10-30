//! Platform seam — every OS-specific path, socket, and per-client location
//! table lives here so the rest of the backend stays platform-neutral.
//!
//! Clash Verge Rev sock locations (unix):
//! - 2.5.6+ service: `/var/run/clash-verge-service/users/<uid>/verge-mihomo.sock`
//! - Legacy: `/tmp/verge/verge-mihomo.sock`
//! - TMPDIR fallback: `$TMPDIR/verge-mihomo.sock` (also written into clash-verge.yaml)
//!
//! Verge config still resolves through `dirs::config_dir()` (macOS Application
//! Support / Windows `%APPDATA%`). Non-macOS client table entries stay conservative.

use std::path::PathBuf;

/// Clash Verge Rev config, relative to the OS config dir.
pub const VERGE_REL_CONFIG: &str = "io.github.clash-verge-rev.clash-verge-rev/config.yaml";

/// Sibling runtime yaml (often fresher EC fields than stale `config.yaml`).
pub const VERGE_REL_CLASH_VERGE_YAML: &str =
    "io.github.clash-verge-rev.clash-verge-rev/clash-verge.yaml";

/// Legacy Verge mihomo controller Unix socket (pre-2.5.6; unix targets only).
pub const VERGE_SOCK_PATH: &str = "/tmp/verge/verge-mihomo.sock";

/// Clash Verge service base dir for per-uid controller socks (2.5.6+).
pub const VERGE_SERVICE_USERS_DIR: &str = "/var/run/clash-verge-service/users";

/// Mihomo Party controller sock (unix only) — consumed via the client table.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))] // 只在 macOS 客户端表中使用
pub const PARTY_SOCK_PATH: &str = "/tmp/mihomo-party.sock";

pub fn verge_config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")))
        .join(VERGE_REL_CONFIG)
}

pub fn verge_clash_verge_yaml_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")))
        .join(VERGE_REL_CLASH_VERGE_YAML)
}

/// Current process UID (unix). Used to prefer the matching Verge service sock.
#[cfg(unix)]
pub fn current_uid() -> u32 {
    extern "C" {
        fn geteuid() -> u32;
    }
    // SAFETY: geteuid is a trivial POSIX getter with no preconditions.
    unsafe { geteuid() }
}

#[cfg(not(unix))]
pub fn current_uid() -> u32 {
    0
}

/// 套接字属主 / 对端 uid 是否可信：本人（euid）或 root（Verge 系统服务）。
/// 别的普通用户一律拒绝，避免把当前用户的控制器 secret 发给他人进程。
#[cfg(unix)]
pub fn sock_owner_trusted(owner_uid: u32, current: u32) -> bool {
    owner_uid == current || owner_uid == 0
}

/// 连接前的路径侧检查（不跟随符号链接）：
/// - 路径本身是 symlink → 拒绝（防止指向他人 sock 的 TOCTOU 替换）
/// - lstat 属主不是本人 / root → 拒绝（发现阶段廉价过滤）
///
/// 真正的信任判定在连接后的 `ensure_connected_peer_trusted`。
#[cfg(unix)]
pub fn ensure_sock_path_trusted(sock: &str) -> Result<(), String> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let meta = std::fs::symlink_metadata(sock).map_err(|e| format!("lstat {sock}: {e}"))?;
    if meta.file_type().is_symlink() {
        return Err(format!("拒绝符号链接套接字路径 {sock}"));
    }
    if !meta.file_type().is_socket() {
        return Err(format!("不是 Unix 套接字：{sock}"));
    }
    let owner = meta.uid();
    let me = current_uid();
    if sock_owner_trusted(owner, me) {
        Ok(())
    } else {
        Err(format!(
            "拒绝连接其他用户拥有的套接字 {sock}（属主 uid {owner}，当前 uid {me}）"
        ))
    }
}

// 非 Unix 平台（Windows）不编译 `ensure_sock_path_trusted`：那里没有 Unix 套接字，
// 旧版的 `Ok(())` 桩等于静默放行。删除后任何调用点在 Windows 上都会编译失败，
// 必须显式处理成「不可用」（见 `mihomo::discovery::select_working_sock`）。
// Windows 的本机 IPC 信任校验走命名管道（M1：`ensure_pipe_server_trusted`）。

/// 连接后检查对端进程凭证：peer uid 必须是本人或 root。
/// Verge service 模式下 sock 文件属主可能是当前用户，但 mihomo 以 root 运行——peer uid=0 应被接受。
#[cfg(unix)]
pub fn ensure_connected_peer_trusted(
    stream: &std::os::unix::net::UnixStream,
    sock: &str,
) -> Result<(), String> {
    use std::os::fd::AsRawFd;
    let peer = peer_uid(stream.as_raw_fd()).map_err(|e| format!("peercred {sock}: {e}"))?;
    let me = current_uid();
    if sock_owner_trusted(peer, me) {
        Ok(())
    } else {
        Err(format!(
            "拒绝连接其他用户进程的套接字 {sock}（对端 uid {peer}，当前 uid {me}）"
        ))
    }
}

#[cfg(unix)]
fn peer_uid(fd: std::os::fd::RawFd) -> Result<u32, String> {
    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd", target_os = "dragonfly"))]
    {
        let mut euid: libc::uid_t = 0;
        let mut egid: libc::gid_t = 0;
        // SAFETY: getpeereid writes into the two out-params; fd is a connected unix socket.
        let rc = unsafe { libc::getpeereid(fd, &mut euid, &mut egid) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(euid as u32)
    }
    #[cfg(target_os = "linux")]
    {
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: getsockopt(SO_PEERCRED) fills ucred for a connected unix socket.
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut cred as *mut _ as *mut libc::c_void,
                &mut len,
            )
        };
        if rc != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(cred.uid as u32)
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly",
        target_os = "linux"
    )))]
    {
        let _ = fd;
        Err("peer credentials unsupported on this OS".into())
    }
}

/// `$TMPDIR/verge-mihomo.sock` (or `/tmp/...` when TMPDIR unset).
pub fn verge_tmpdir_sock() -> String {
    let tmp = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into());
    let tmp = tmp.trim_end_matches('/');
    format!("{tmp}/verge-mihomo.sock")
}

/// Service sock for the current user: `/var/run/clash-verge-service/users/<uid>/verge-mihomo.sock`.
pub fn verge_service_sock_for_uid(uid: u32) -> String {
    format!("{VERGE_SERVICE_USERS_DIR}/{uid}/verge-mihomo.sock")
}

/// Ordered Verge sock candidates (deduped). `yaml_unix_paths` are already expanded.
///
/// Order: service (current uid only) → legacy
/// `/tmp/verge/...` → `$TMPDIR/...` → yaml unix paths → optional extras.
pub fn verge_sock_candidates(yaml_unix_paths: &[String], extras: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |s: String| {
        if s.is_empty() {
            return;
        }
        if !out.iter().any(|e| e == &s) {
            out.push(s);
        }
    };

    if cfg!(unix) {
        let uid = current_uid();
        push(verge_service_sock_for_uid(uid));

        // 0.2.9：不再枚举 service 目录下其他 uid 的套接字——那是别的用户的 mihomo，
        // 不应把当前用户的 secret 发过去（见 `ensure_sock_path_trusted`）。

        push(VERGE_SOCK_PATH.to_string());
        push(verge_tmpdir_sock());
    }

    for p in yaml_unix_paths {
        push(p.clone());
    }
    for p in extras {
        push(p.clone());
    }
    out
}

#[cfg(target_os = "macos")]
pub fn app_log_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join("Library/Logs/ClashAssist"))
}

#[cfg(not(target_os = "macos"))]
pub fn app_log_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("logs").join("ClashAssist"))
}

/// Per-client controller discovery inputs. `yaml_paths` may use `~` and may be
/// directories (scanned for *.yaml / *.yml by `read_controller_yaml`).
pub struct ClientDiscovery {
    pub yaml_paths: Vec<String>,
    pub default_port: u16,
    pub default_mixed: u16,
    pub sock_candidates: Vec<String>,
    pub source_prefix: &'static str,
}

/// FlClash preference files (macOS NSUserDefaults `.plist` + shared_preferences JSON).
/// Modern FlClash (v0.9+) stores `patchClashConfig` (`external-controller`, `secret`,
/// `mixed-port`) in `flutter.config` inside `~/Library/Preferences/com.follow.clash.plist`
/// or `shared_preferences.json`, and may skip writing `config.yaml` on base-profile setups.
#[cfg(target_os = "macos")]
pub fn flclash_preference_paths() -> Vec<String> {
    vec![
        "~/Library/Preferences/com.follow.clash.plist".into(),
        "~/Library/Containers/com.follow.clash/Data/Library/Preferences/com.follow.clash.plist".into(),
        "~/Library/Application Support/com.follow.clash/shared_preferences.json".into(),
        "~/Library/Application Support/FlClash/shared_preferences.json".into(),
    ]
}

#[cfg(not(target_os = "macos"))]
pub fn flclash_preference_paths() -> Vec<String> {
    vec![
        "~/Library/Application Support/com.follow.clash/shared_preferences.json".into(),
        "~/Library/Application Support/FlClash/shared_preferences.json".into(),
    ]
}

/// macOS table — must stay identical to the historical hardcoded paths.
/// Verge is NOT here: it goes through `discover_controller()` in `mihomo`.
#[cfg(target_os = "macos")]
pub fn client_discovery(client_id: &str) -> Option<ClientDiscovery> {
    let spec = match client_id {
        "clashx_meta" => ClientDiscovery {
            yaml_paths: vec!["~/.config/clash/config.yaml".into()],
            default_port: 9090,
            default_mixed: 7890,
            sock_candidates: vec![], // no fixed public sock
            source_prefix: "clashx_meta",
        },
        "flclash" => ClientDiscovery {
            yaml_paths: vec![
                "~/Library/Application Support/com.follow.clash/config.yaml".into(),
                "~/Library/Application Support/FlClash/config.yaml".into(),
                "~/Library/Application Support/com.follow.clash".into(),
                "~/Library/Application Support/FlClash".into(),
            ],
            default_port: 9090,
            default_mixed: 7890,
            // internal IPC is not Mihomo REST — never inject Verge/Party sock
            sock_candidates: vec![],
            source_prefix: "flclash",
        },
        "mihomo_party" => ClientDiscovery {
            yaml_paths: vec!["~/Library/Application Support/mihomo-party".into()],
            default_port: 9090, // TCP EC often empty; sock is primary
            default_mixed: 7890,
            sock_candidates: vec![PARTY_SOCK_PATH.into()],
            source_prefix: "mihomo_party",
        },
        "nyanpasu" => ClientDiscovery {
            yaml_paths: vec![
                "~/Library/Application Support/Clash Nyanpasu/clash-runtime.yaml".into(),
                "~/Library/Application Support/Clash Nyanpasu/clash.yaml".into(),
                "~/Library/Application Support/clash-nyanpasu/clash-runtime.yaml".into(),
                "~/Library/Application Support/clash-nyanpasu/clash.yaml".into(),
                "~/Library/Application Support/Clash Nyanpasu".into(),
                "~/Library/Application Support/clash-nyanpasu".into(),
            ],
            default_port: 17650, // default EC; debug builds may use 9872
            default_mixed: 7890,
            sock_candidates: vec![],
            source_prefix: "nyanpasu",
        },
        _ => return None,
    };
    Some(spec)
}

/// Non-macOS placeholder table. Only `clashx_meta`'s Linux-style path is kept
/// (it is valid on Linux); other clients get TCP defaults only until a real
/// Windows/Linux port tunes their locations.
#[cfg(not(target_os = "macos"))]
pub fn client_discovery(client_id: &str) -> Option<ClientDiscovery> {
    let spec = match client_id {
        "clashx_meta" => ClientDiscovery {
            yaml_paths: vec!["~/.config/clash/config.yaml".into()],
            default_port: 9090,
            default_mixed: 7890,
            sock_candidates: vec![],
            source_prefix: "clashx_meta",
        },
        "flclash" | "mihomo_party" | "nyanpasu" => ClientDiscovery {
            yaml_paths: vec![],
            default_port: 9090,
            default_mixed: 7890,
            sock_candidates: vec![],
            source_prefix: "placeholder",
        },
        _ => return None,
    };
    Some(spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verge_config_path_unifies_via_config_dir() {
        let p = verge_config_path();
        assert!(
            p.to_string_lossy().ends_with(VERGE_REL_CONFIG),
            "unexpected path: {p:?}"
        );
        // macOS must keep the exact historical location.
        #[cfg(target_os = "macos")]
        {
            let home = dirs::home_dir().expect("home dir");
            assert_eq!(
                p,
                home.join("Library/Application Support").join(VERGE_REL_CONFIG),
                "macOS Verge config path must be byte-identical to pre-seam code"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn sock_owner_policy() {
        assert!(sock_owner_trusted(501, 501));
        assert!(sock_owner_trusted(0, 501));
        assert!(!sock_owner_trusted(502, 501));
        assert!(sock_owner_trusted(0, 0));
    }

    #[cfg(unix)]
    #[test]
    fn own_socket_path_is_trusted_and_candidates_skip_other_uids() {
        let dir = std::env::temp_dir().join(format!("ca-sock-owner-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("m.sock");
        let _l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        assert!(ensure_sock_path_trusted(sock.to_str().unwrap()).is_ok());
        assert!(ensure_sock_path_trusted("/nonexistent/ca.sock").is_err());
        let _ = std::fs::remove_dir_all(&dir);

        let me = current_uid();
        for c in verge_sock_candidates(&[], &[]) {
            if let Some(rest) = c.strip_prefix(&format!("{VERGE_SERVICE_USERS_DIR}/")) {
                assert!(rest.starts_with(&format!("{me}/")), "unexpected other-uid candidate {c}");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn connected_peer_of_own_listener_is_trusted() {
        use std::os::unix::net::{UnixListener, UnixStream};
        let dir = std::env::temp_dir().join(format!("ca-peercred-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("p.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let path = sock.to_str().unwrap().to_string();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
        });
        let client = UnixStream::connect(&path).unwrap();
        assert!(ensure_connected_peer_trusted(&client, &path).is_ok());
        let _ = server.join();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_sock_path_is_rejected() {
        use std::os::unix::net::UnixListener;
        let dir = std::env::temp_dir().join(format!("ca-sock-symlink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let real = dir.join("real.sock");
        let link = dir.join("link.sock");
        let _l = UnixListener::bind(&real).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let err = ensure_sock_path_trusted(link.to_str().unwrap()).expect_err("symlink must fail");
        assert!(err.contains("符号链接") || err.to_lowercase().contains("symlink"), "{err}");
        assert!(ensure_sock_path_trusted(real.to_str().unwrap()).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn verge_sock_candidates_order_and_dedupe() {
        let yaml = vec![
            VERGE_SOCK_PATH.to_string(),
            "/custom/from-yaml.sock".to_string(),
        ];
        let c = verge_sock_candidates(&yaml, &[]);
        if cfg!(unix) {
            assert_eq!(c[0], verge_service_sock_for_uid(current_uid()));
            assert!(c.iter().any(|p| p == VERGE_SOCK_PATH));
            assert!(c.iter().any(|p| p == &verge_tmpdir_sock()));
            assert!(c.iter().any(|p| p == "/custom/from-yaml.sock"));
            // legacy path appears only once even though also in yaml
            assert_eq!(c.iter().filter(|p| p.as_str() == VERGE_SOCK_PATH).count(), 1);
        } else {
            assert_eq!(c, vec!["/custom/from-yaml.sock".to_string()]);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_client_table_preserves_historical_values() {
        let cx = client_discovery("clashx_meta").expect("clashx_meta");
        assert_eq!(cx.yaml_paths, vec!["~/.config/clash/config.yaml".to_string()]);
        assert_eq!(cx.default_port, 9090);
        assert_eq!(cx.default_mixed, 7890);
        assert!(cx.sock_candidates.is_empty());

        let fl = client_discovery("flclash").expect("flclash");
        assert_eq!(
            fl.yaml_paths,
            vec![
                "~/Library/Application Support/com.follow.clash/config.yaml".to_string(),
                "~/Library/Application Support/FlClash/config.yaml".to_string(),
                "~/Library/Application Support/com.follow.clash".to_string(),
                "~/Library/Application Support/FlClash".to_string(),
            ]
        );
        assert!(fl.sock_candidates.is_empty());

        let party = client_discovery("mihomo_party").expect("mihomo_party");
        assert_eq!(party.sock_candidates, vec![PARTY_SOCK_PATH.to_string()]);

        let ny = client_discovery("nyanpasu").expect("nyanpasu");
        assert_eq!(ny.default_port, 17650);
        assert_eq!(ny.yaml_paths.len(), 6);

        // Verge is handled by discover_controller, not the table.
        assert!(client_discovery("verge").is_none());
        assert!(client_discovery("nope").is_none());
    }
}
