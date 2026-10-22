#[cfg(target_os = "macos")]
mod app_menu;
mod cli_shim;
mod cli_serve;
mod dns;
mod dns_query;
mod mihomo;
mod platform;
mod update_check;

use dns::DnsResolversResult;
use mihomo::{
    discover_controller, discover_for_client, http_via_tcp_async, http_via_unix, list_nodes_async,
    arm_cancel, disarm_cancel, fire_cancel, proxy_fetch_async, proxy_timed_transfer_async,
    DiscoverResult, ListNodesResult, TimedTransferResult, UnixHttpResult,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControllerConfig {
    pub host: String,
    pub port: u16,
    pub secret: String,
    pub mixed_port: u16,
    pub source: String,
    pub sock_path: Option<String>,
}

fn join_err(e: impl std::fmt::Display) -> String {
    format!("task join: {e}")
}

fn catch_disk<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + std::panic::UnwindSafe,
{
    match std::panic::catch_unwind(f) {
        Ok(r) => r,
        Err(_) => Err("native panic caught (disk/unix path)".into()),
    }
}

#[tauri::command]
async fn discover_mihomo() -> Result<ControllerConfig, String> {
    tauri::async_runtime::spawn_blocking(|| {
        catch_disk(|| {
            let d: DiscoverResult = discover_controller();
            Ok(ControllerConfig {
                host: d.host,
                port: d.port,
                secret: d.secret,
                mixed_port: d.mixed_port,
                source: d.source,
                sock_path: d.sock_path,
            })
        })
    })
    .await
    .map_err(join_err)?
}


#[tauri::command]
async fn discover_mihomo_for_client(client_id: String) -> Result<ControllerConfig, String> {
    tauri::async_runtime::spawn_blocking(move || {
        catch_disk(|| {
            let d: DiscoverResult = discover_for_client(&client_id);
            Ok(ControllerConfig {
                host: d.host,
                port: d.port,
                secret: d.secret,
                mixed_port: d.mixed_port,
                source: d.source,
                sock_path: d.sock_path,
            })
        })
    })
    .await
    .map_err(join_err)?
}

#[tauri::command]
async fn read_verge_config_raw() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(|| {
        catch_disk(|| mihomo::read_verge_config().map_err(|e| e.to_string()))
    })
    .await
    .map_err(join_err)?
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TcpHttpRequest {
    host: String,
    port: u16,
    method: String,
    path: String,
    body: Option<String>,
    secret: String,
    timeout_ms: Option<u64>,
}

#[tauri::command]
async fn mihomo_http(req: TcpHttpRequest) -> Result<UnixHttpResult, String> {
    http_via_tcp_async(
        &req.host,
        req.port,
        &req.method,
        &req.path,
        req.body.as_deref(),
        &req.secret,
        req.timeout_ms.unwrap_or(3000),
    )
    .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListNodesRequest {
    host: String,
    port: u16,
    secret: String,
    timeout_ms: Option<u64>,
    sock_path: Option<String>,
}

#[tauri::command]
async fn mihomo_list_nodes(req: ListNodesRequest) -> Result<ListNodesResult, String> {
    list_nodes_async(
        &req.host,
        req.port,
        &req.secret,
        req.timeout_ms.unwrap_or(18000),
        req.sock_path.as_deref(),
    )
    .await
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UnixHttpRequest {
    method: String,
    path: String,
    body: Option<String>,
    secret: String,
    sock_path: Option<String>,
    timeout_ms: Option<u64>,
}

#[tauri::command]
async fn mihomo_unix_http(req: UnixHttpRequest) -> Result<UnixHttpResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        catch_disk(|| {
            http_via_unix(
                &req.method,
                &req.path,
                req.body.as_deref(),
                &req.secret,
                req.sock_path.as_deref(),
                req.timeout_ms.unwrap_or(3000),
            )
            .map_err(|e| e.to_string())
        })
    })
    .await
    .map_err(join_err)?
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProxyFetchRequest {
    url: String,
    mixed_port: Option<u16>,
    user_agent: Option<String>,
    timeout_ms: Option<u64>,
    cancel_id: Option<String>,
}

#[tauri::command]
async fn egress_proxy_fetch(req: ProxyFetchRequest) -> Result<UnixHttpResult, String> {
    let cancel = match req.cancel_id.as_deref() {
        Some(id) => match arm_cancel(id) {
            Some(rx) => Some(rx),
            None => return Err("已取消".into()),
        },
        None => None,
    };
    let result = proxy_fetch_async(
        &req.url,
        req.mixed_port,
        req.user_agent.as_deref(),
        req.timeout_ms.unwrap_or(5000),
        cancel,
    )
    .await;
    if let Some(id) = req.cancel_id.as_deref() {
        disarm_cancel(id);
    }
    result
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CancelProxyOpRequest {
    cancel_id: String,
}

#[tauri::command]
fn egress_cancel_proxy_op(req: CancelProxyOpRequest) -> bool {
    fire_cancel(&req.cancel_id)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TimedTransferRequest {
    url: String,
    mixed_port: Option<u16>,
    method: Option<String>,
    upload_bytes: Option<u64>,
    timeout_ms: Option<u64>,
    cancel_id: Option<String>,
}

#[tauri::command]
async fn egress_proxy_timed_transfer(
    req: TimedTransferRequest,
) -> Result<TimedTransferResult, String> {
    let cancel = match req.cancel_id.as_deref() {
        Some(id) => match arm_cancel(id) {
            Some(rx) => Some(rx),
            None => {
                return Ok(TimedTransferResult {
                    ok: false,
                    status: 0,
                    bytes: 0,
                    elapsed_ms: 0,
                    error: Some("已取消".into()),
                });
            }
        },
        None => None,
    };
    let result = proxy_timed_transfer_async(
        &req.url,
        req.mixed_port,
        req.method.as_deref().unwrap_or("GET"),
        req.upload_bytes,
        req.timeout_ms.unwrap_or(12000),
        cancel,
    )
    .await;
    if let Some(id) = req.cancel_id.as_deref() {
        disarm_cancel(id);
    }
    result
}

#[tauri::command]
async fn egress_list_dns_resolvers() -> Result<DnsResolversResult, String> {
    tauri::async_runtime::spawn_blocking(|| {
        catch_disk(|| Ok(dns::list_dns_resolvers_blocking()))
    })
    .await
    .map_err(join_err)?
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DnsWhoamiRequest {
    /// Force the query through a specific recursive resolver (e.g. "8.8.8.8");
    /// None/empty uses the system default path.
    resolver: Option<String>,
    timeout_ms: Option<u64>,
    /// 卡片到点时前端拿这个号把还在等的域名查询停掉。
    cancel_id: Option<String>,
}

#[tauri::command]
async fn egress_dns_whoami(req: DnsWhoamiRequest) -> Result<dns::DnsWhoamiResult, String> {
    let resolver = req.resolver.filter(|s| !s.trim().is_empty());
    let timeout_ms = req.timeout_ms.unwrap_or(4000);
    let cancel_id = req.cancel_id.filter(|s| !s.trim().is_empty());
    let armed = cancel_id.as_deref().map(arm_cancel);
    if matches!(&armed, Some(None)) {
        return Ok(dns::DnsWhoamiResult {
            ok: false,
            client_ip: None,
            resolver_ns: None,
            ecs: None,
            via: "system".into(),
            raw: String::new(),
            error: Some("已取消".into()),
        });
    }
    let wait = armed.flatten();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let wait = std::panic::AssertUnwindSafe(wait);
        catch_disk(move || {
            let cancelled = || wait.as_ref().map(|w| w.is_cancelled()).unwrap_or(false);
            Ok(dns::dns_whoami_blocking(
                resolver.as_deref(),
                timeout_ms,
                &cancelled,
            ))
        })
    })
    .await
    .map_err(join_err)?;
    if let Some(id) = cancel_id.as_deref() {
        disarm_cancel(id);
    }
    result
}


/// Append a line to the platform log dir (macOS: ~/Library/Logs/ClashAssist). Best-effort.
fn append_app_log(msg: &str) {
    use std::io::Write;

    let Some(dir) = platform::app_log_dir() else {
        return;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join("app.log");
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    else {
        return;
    };
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let _ = writeln!(f, "ts_unix={secs} {msg}");
}

#[cfg(debug_assertions)]
async fn vite_dev_reachable(url: &str) -> bool {
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(600))
        .no_proxy()
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    // Any HTTP response (incl. 4xx) means something is listening on the devUrl.
    client.get(url).send().await.is_ok()
}

/// Inject a full-page Chinese error into the live WebView. Never uses data: navigate
/// (that path crashed WKWebView on macOS).
#[cfg(debug_assertions)]
fn show_vite_dead_page(win: &tauri::WebviewWindow, reason: &str) {
    let _ = win.set_title("Clash伴侣 — Vite 已退出");
    // Build DOM via JS literals; avoid navigating away from the webview origin.
    let js = format!(
        r#"(function(){{
  if (window.__egressViteDeadShown) return;
  window.__egressViteDeadShown = true;
  try {{
    document.documentElement.style.background = '#0f1419';
    document.body.style.margin = '0';
    document.body.style.background = '#0f1419';
    document.body.style.color = '#e7ecf3';
    document.body.innerHTML = '';
    var wrap = document.createElement('div');
    wrap.setAttribute('id', 'vite-dead');
    wrap.style.cssText = 'font-family:-apple-system,BlinkMacSystemFont,\"PingFang SC\",\"Helvetica Neue\",sans-serif;max-width:560px;margin:64px auto;padding:24px;line-height:1.6;color:#e7ecf3;background:#0f1419';
    wrap.innerHTML = '<h1 style="font-size:22px;color:#ffb454;margin:0 0 12px">Vite 已退出，前端不可用</h1>'
      + '<p>开发服务器（127.0.0.1:1420）已停止或无法连接，所以只剩空白窗口。</p>'
      + '<p>请<strong>关掉本窗口</strong>，然后只开一个终端重新运行：</p>'
      + '<p><code style="background:#1c2430;padding:2px 8px;border-radius:4px">pnpm tauri dev</code></p>'
      + '<p style="opacity:.75;font-size:13px">不要直接打开 target/debug/clash-assist（会留下无前端的空壳）。</p>'
      + '<pre style="white-space:pre-wrap;font-size:12px;opacity:.7;margin-top:16px"></pre>';
    document.body.appendChild(wrap);
    var pre = wrap.querySelector('pre');
    if (pre) pre.textContent = {reason_json};
  }} catch (e) {{}}
}})();"#,
        reason_json = serde_json::to_string(reason).unwrap_or_else(|_| "\"vite unreachable\"".into())
    );
    let _ = win.eval(&js);
}


/// CLI mode: argv as seen by the process (includes binary path).
#[tauri::command]
fn get_cli_argv() -> Vec<String> {
    std::env::args().collect()
}

/// CLI 子命令全集。
/// ⚠️ 必须与 TS `src/lib/cli/parse.ts` 的 `CLI_COMMANDS` 完全一致（cli.test.ts 有同步断言）。
pub const CLI_VERBS: &[&str] = &[
    "help",
    "discover",
    "nodes",
    "gate",
    "check",
    "env",
    "serve",
    "install-shim",
    "update-check",
];

/// 是否进入 CLI（隐藏主窗）。规则与 TS `parseCliArgv().cliMode` 逐条对齐：
/// - 选项区出现 `--cli` / `--help` / `-h` → 是
/// - 否则看第一个位置参数是否为已知子命令
/// - `--client` / `-c` / `--port` / `-p` 后面的值不算位置参数（值缺失或以 `-` 开头时不吞）
/// - `--` 之后全部是位置参数（`--cli` 也不再算选项）
///
/// 共用用例：`src/lib/cli/cli-mode-cases.json`（Rust 单测 + vitest 都跑）。
pub fn is_cli_mode_args(args: &[String]) -> bool {
    let mut flag = false;
    let mut first_positional: Option<&str> = None;
    let mut end_of_options = false;
    let mut i = 1; // args[0] = 二进制路径
    while i < args.len() {
        let t = args[i].as_str();
        if end_of_options || !t.starts_with('-') || t == "-" {
            if first_positional.is_none() {
                first_positional = Some(t);
            }
            i += 1;
            continue;
        }
        match t {
            "--" => end_of_options = true,
            "--cli" | "--help" | "-h" => flag = true,
            "--client" | "-c" | "--port" | "-p" => {
                if let Some(v) = args.get(i + 1) {
                    if !v.is_empty() && !v.starts_with('-') {
                        i += 2;
                        continue;
                    }
                }
            }
            _ => {
                // --port=N / --client=verge：值贴在选项上，不算位置参数
            }
        }
        i += 1;
    }
    flag || first_positional.is_some_and(|p| CLI_VERBS.contains(&p))
}

/// True when process was launched in CLI mode (see `is_cli_mode_args`).
#[tauri::command]
fn is_cli_mode() -> bool {
    let args: Vec<String> = std::env::args().collect();
    is_cli_mode_args(&args)
}

/// CLI 中断（Ctrl+C / SIGTERM / SIGHUP）。
///
/// 第一次信号：只置位，由前端轮询 `cli_abort_requested` 后让 `check all`
/// 在节点间停止并切回原节点，正常输出 envelope（error.code = aborted）后退出 1。
/// 看门狗：第一次信号后 `CLI_ABORT_GRACE_SECS` 秒仍未退出 → 强制退出 130。
/// 第二次信号：立即 `_exit(130)`，不切回。
#[cfg(unix)]
mod cli_signal {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SIGNALS: AtomicUsize = AtomicUsize::new(0);
    pub const CLI_ABORT_GRACE_SECS: u64 = 90;
    const FORCE_MSG: &str =
        "\n再次收到中断：立即退出（没有切回原节点，请到代理软件里确认当前节点）。\n";

    extern "C" fn on_signal(_sig: libc::c_int) {
        let prev = SIGNALS.fetch_add(1, Ordering::SeqCst);
        if prev >= 1 {
            // 信号处理函数里只用 async-signal-safe 调用：write + _exit。
            let b = FORCE_MSG.as_bytes();
            unsafe {
                libc::write(2, b.as_ptr() as *const libc::c_void, b.len());
                libc::_exit(130);
            }
        }
    }

    pub fn requested() -> bool {
        SIGNALS.load(Ordering::SeqCst) > 0
    }

    pub fn install() {
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
            sa.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut sa.sa_mask);
            for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
                if libc::sigaction(sig, &sa, std::ptr::null_mut()) != 0 {
                    eprintln!("cli: 安装信号处理失败（signal {sig}），Ctrl+C 将直接结束进程");
                }
            }
        }
        std::thread::spawn(|| {
            while !requested() {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            eprintln!(
                "收到中断信号：正在停止（切换过节点的会先切回原节点，最多等 {CLI_ABORT_GRACE_SECS} 秒；再按一次 Ctrl+C 立即退出，不切回）…"
            );
            std::thread::sleep(std::time::Duration::from_secs(CLI_ABORT_GRACE_SECS));
            eprintln!(
                "等待切回超时（{CLI_ABORT_GRACE_SECS} 秒），强制退出。请到代理软件里确认当前节点。"
            );
            std::process::exit(130);
        });
    }
}

/// 前端轮询：是否已收到中断信号。
#[tauri::command]
fn cli_abort_requested() -> bool {
    #[cfg(unix)]
    {
        cli_signal::requested()
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Print one line to process stdout (JSON envelope or help text).
#[tauri::command]
fn cli_stdout(line: String) -> Result<(), String> {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    writeln!(out, "{line}").map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())?;
    Ok(())
}


/// Print one line (or multi-line) to process stderr (serve 就绪提示等).
#[tauri::command]
fn cli_stderr(line: String) -> Result<(), String> {
    use std::io::Write;
    let mut out = std::io::stderr().lock();
    writeln!(out, "{line}").map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())?;
    Ok(())
}

/// macOS 版本（CLI 在 < 14 上 check/env 会卡死，前端据此快速失败）。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CliOsInfo {
    os: &'static str,
    version: Option<String>,
    major: Option<u32>,
}

#[tauri::command]
fn cli_os_info() -> CliOsInfo {
    let version = cli_shim::macos_version();
    let major = version.as_deref().and_then(cli_shim::major_of);
    CliOsInfo {
        os: std::env::consts::OS,
        version,
        major,
    }
}

/// 更新检查（GUI 启动时一次 + CLI update-check）。失败返回 Err，由调用方决定是否静默。
#[tauri::command]
async fn check_update() -> Result<update_check::UpdateInfo, String> {
    update_check::check(env!("CARGO_PKG_VERSION")).await
}

/// 系统首选语言标签（前端「跟随系统」时据此选 简体 / 繁體 / English）。
#[tauri::command]
fn system_locale() -> String {
    app_menu::system_locale_tag()
}

/// 前端切换界面语言（"system" / "zh-Hans" / "zh-Hant" / "en"）：覆盖菜单语言并重建 macOS 菜单。
#[tauri::command]
fn set_ui_language(app: tauri::AppHandle, lang: String) -> Result<(), String> {
    let next = app_menu::parse_pref(&lang);
    app_menu::set_override(next);
    let menu = app_menu::build(&app).map_err(|e| e.to_string())?;
    app.set_menu(menu).map(|_| ()).map_err(|e| e.to_string())
}

/// 用默认浏览器打开 release 页面（只接受本项目 GitHub releases 下的 URL）。
#[tauri::command]
fn open_release_page(url: String) -> Result<(), String> {
    update_check::open_release_page(&url)
}

/// 应用内自更新阶段 1：下载最新 .dmg 到临时目录并校验 SHA-256。
#[tauri::command]
async fn download_update(dmg_url: Option<String>, dmg_sha256: Option<String>) -> Result<String, String> {
    update_check::download_to_staging(dmg_url, dmg_sha256).await
}

/// 应用内自更新阶段 2：后台静默挂载 .dmg、清除隔离属性并替换当前 .app（不强行退出）。
#[tauri::command]
async fn apply_update(work_dir: String) -> Result<String, String> {
    update_check::apply_staged_update(work_dir).await
}

/// 应用内自更新阶段 3：用户点击「立即重启」时拉起新版 .app 并退出当前进程。
#[tauri::command]
fn relaunch_updated_app() -> Result<(), String> {
    update_check::relaunch_updated_app()
}

/// `--cli install-shim [--dir D]`
#[tauri::command]
fn cli_install_shim(dir: Option<String>) -> Result<cli_shim::ShimResult, cli_shim::ShimError> {
    cli_shim::install(dir.as_deref())
}

#[tauri::command]
fn cli_serve_start(port: u16) -> Result<cli_serve::ServeStartInfo, String> {
    cli_serve::serve_start(port)
}

#[tauri::command]
fn cli_serve_poll(timeout_ms: u64) -> Result<Option<cli_serve::ServeJobView>, String> {
    cli_serve::serve_poll(timeout_ms)
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServeRespondRequest {
    id: u64,
    status: u16,
    body: String,
}

#[tauri::command]
fn cli_serve_respond(req: ServeRespondRequest) -> Result<(), String> {
    cli_serve::serve_respond(req.id, req.status, req.body)
}

#[tauri::command]
fn cli_serve_stop() -> Result<(), String> {
    cli_serve::serve_stop()
}

/// Exit the process after CLI finishes (code 0 = ok envelope, 1 = error).
#[tauri::command]
fn cli_exit(code: i32) -> Result<(), String> {
    // Give stdout a tick to flush on some terminals.
    let _ = std::io::Write::flush(&mut std::io::stdout());
    std::process::exit(code);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// 0.2.0 起 bundle identifier 由 com.egresschecker.app 改为 com.clashassist.app。
/// 首次启动：新目录不存在或为空、而旧目录存在时，把旧目录整体复制过来（不删除旧目录）。
/// 覆盖 ~/Library/Application Support、~/Library/Caches 下的应用数据，
/// 以及 ~/Library/WebKit/<id>（前端设置存在 WKWebView 的 localStorage 里）。
/// 必须在主 webview 创建前调用。Best-effort，失败只记日志。
const LEGACY_IDENTIFIER: &str = "com.egresschecker.app";

fn dir_is_empty_or_missing(p: &std::path::Path) -> bool {
    match std::fs::read_dir(p) {
        Ok(mut it) => it.next().is_none(),
        Err(_) => true,
    }
}

fn copy_dir_all(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &to)?;
        } else if ty.is_file() {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

fn migrate_legacy_identifier_dirs(new_id: &str) {
    if new_id == LEGACY_IDENTIFIER {
        return;
    }
    let mut roots: Vec<std::path::PathBuf> = Vec::new();
    if let Some(d) = dirs::data_dir() {
        roots.push(d);
    }
    if let Some(d) = dirs::config_dir() {
        roots.push(d);
    }
    if let Some(d) = dirs::cache_dir() {
        roots.push(d);
    }
    if let Some(h) = dirs::home_dir() {
        roots.push(h.join("Library/WebKit"));
    }
    roots.dedup();
    for root in roots {
        let old = root.join(LEGACY_IDENTIFIER);
        let new = root.join(new_id);
        if old.is_dir() && dir_is_empty_or_missing(&new) {
            match copy_dir_all(&old, &new) {
                Ok(()) => append_app_log(&format!("migrated {} -> {}", old.display(), new.display())),
                Err(e) => append_app_log(&format!("migrate {} failed: {e}", old.display())),
            }
        }
    }
}

pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(target_os = "macos")]
    let builder = builder.menu(app_menu::build).on_menu_event(|app, event| {
        if event.id() == "reload_window" {
            use tauri::Manager;
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.eval("window.location.reload()");
            }
        }
    });
    builder
        .setup(|app| {
            use tauri::Manager;
            migrate_legacy_identifier_dirs(&app.config().identifier);
            let frontend_url = app
                .config()
                .build
                .dev_url
                .as_ref()
                .map(|u| u.to_string())
                .unwrap_or_else(|| "http://127.0.0.1:1420".into());
            append_app_log(&format!(
                "startup frontend_url={frontend_url} (no data-url navigate)"
            ));

            // 主窗口在这里创建（tauri.conf.json 里 create:false），以便 CLI 模式单独配置：
            // - visible(false)：一开始就不显示，不再先闪一下再隐藏；
            // - 关闭后台节流：WKWebView 默认把不在屏幕上的页面「挂起」（setTimeout 等不再触发），
            //   隐藏窗里的深测会永远卡住（0.1.15 及以前 `--cli check current/all` 不返回）。
            //   该设置需 macOS 14+（更早系统 wry 会忽略）。
            // GUI 模式完全按配置创建，行为不变。
            let cli = is_cli_mode();
            let win_cfg = app
                .config()
                .app
                .windows
                .first()
                .cloned()
                .ok_or("tauri.conf.json 缺少主窗口配置")?;
            let mut builder = tauri::WebviewWindowBuilder::from_config(app.handle(), &win_cfg)?;
            if cli {
                builder = builder
                    .visible(false)
                    .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled);
            }
            builder.build()?;

            // 原生标题栏：软件名 + 版本（与 Cargo.toml / package.json 对齐）。
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.set_title(&format!(
                    "Clash伴侣 {}",
                    env!("CARGO_PKG_VERSION")
                ));
            }

            // --cli：主窗已隐藏创建，由前端跑完后 cli_exit；接管 Ctrl+C 以便先切回再退出。
            if cli {
                append_app_log("cli_mode=1 hidden window, background throttling disabled");
                #[cfg(unix)]
                cli_signal::install();
            }

            #[cfg(debug_assertions)]
            {
                if !cli {
                let handle = app.handle().clone();
                let probe_url = frontend_url.clone();
                tauri::async_runtime::spawn(async move {
                    // Grace: Vite/beforeDevCommand needs time to bind.
                    let _ = tauri::async_runtime::spawn_blocking(|| {
                        std::thread::sleep(std::time::Duration::from_secs(3));
                    })
                    .await;

                    let mut ever_ok = false;
                    let mut dead_notified = false;
                    let mut consecutive_fail: u32 = 0;
                    loop {
                        let ok = vite_dev_reachable(&probe_url).await;
                        if ok {
                            ever_ok = true;
                            consecutive_fail = 0;
                        } else {
                            consecutive_fail = consecutive_fail.saturating_add(1);
                            // Require a few misses to avoid race flapping; if never came up,
                            // notify after ~6s of fails (3 probes); if it died after OK, after 2 probes.
                            let threshold = if ever_ok { 2 } else { 3 };
                            if consecutive_fail >= threshold && !dead_notified {
                                dead_notified = true;
                                let reason = if ever_ok {
                                    format!(
                                        "Vite was up then became unreachable at {probe_url}. beforeDevCommand child likely exited; WebView left empty."
                                    )
                                } else {
                                    format!(
                                        "Vite never became reachable at {probe_url}. Do not open bare target/debug; use pnpm tauri dev."
                                    )
                                };
                                append_app_log(&format!("vite_dead {reason}"));
                                if let Some(win) = handle.get_webview_window("main") {
                                    show_vite_dead_page(&win, &reason);
                                }
                            }
                        }
                        let _ = tauri::async_runtime::spawn_blocking(|| {
                            std::thread::sleep(std::time::Duration::from_secs(2));
                        })
                        .await;
                    }
                });
                } // !is_cli_mode
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            discover_mihomo,
            discover_mihomo_for_client,
            read_verge_config_raw,
            mihomo_http,
            mihomo_list_nodes,
            mihomo_unix_http,
            egress_proxy_fetch,
            egress_proxy_timed_transfer,
            egress_cancel_proxy_op,
            egress_list_dns_resolvers,
            egress_dns_whoami,
            get_cli_argv,
            is_cli_mode,
            cli_stdout,
            cli_stderr,
            cli_exit,
            cli_abort_requested,
            cli_os_info,
            cli_install_shim,
            cli_serve_start,
            cli_serve_poll,
            cli_serve_respond,
            cli_serve_stop,
            check_update,
            open_release_page,
            download_update,
            apply_update,
            relaunch_updated_app,
            system_locale,
            set_ui_language
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod cli_mode_tests {
    use super::is_cli_mode_args;

    /// 与 vitest 共用同一份用例，保证 Rust / TS 判定一致。
    #[test]
    fn cli_mode_matches_shared_cases() {
        let raw = include_str!("../../src/lib/cli/cli-mode-cases.json");
        let cases: Vec<serde_json::Value> = serde_json::from_str(raw).expect("cases json");
        assert!(!cases.is_empty());
        for c in cases {
            let argv: Vec<String> = c["argv"]
                .as_array()
                .expect("argv")
                .iter()
                .map(|v| v.as_str().expect("argv str").to_string())
                .collect();
            let want = c["cliMode"].as_bool().expect("cliMode");
            assert_eq!(is_cli_mode_args(&argv), want, "argv={argv:?}");
        }
    }
}
