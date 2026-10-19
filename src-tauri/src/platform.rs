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
/// Order: service (current uid → other uids under the service dir) → legacy
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

        // Sibling uids under the service dir (best-effort; ignore permission errors).
        if let Ok(entries) = std::fs::read_dir(VERGE_SERVICE_USERS_DIR) {
            let mut others: Vec<String> = entries
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    let name = e.file_name().into_string().ok()?;
                    let other_uid: u32 = name.parse().ok()?;
                    if other_uid == uid {
                        return None;
                    }
                    Some(verge_service_sock_for_uid(other_uid))
                })
                .collect();
            others.sort();
            for p in others {
                push(p);
            }
        }

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
