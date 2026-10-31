//! Windows 本地控制通道的信任判定（windows-plan r4 §2.1 规则 4、§2.2 L0–L3；
//! 评审项 M-3、C-1、C-4，Ellie V5 评审 F1–F5）。
//!
//! 这里只有**纯判定**：所有 OS 事实（属主 SID、DACL、服务端 PID、映像名、令牌用户、
//! WTS、TCP 监听表）都经 [`PipeFacts`] / [`ProcessFacts`] 注入，本模块不做任何系统调用。
//! 因此每条规则分支都能在任何平台上用假事实做单元测试（F1）；Windows 上的真实采集见
//! `crate::win`，带真实管道 / 监听服务端的集成测试见 `mihomo::pipe` 与 `mihomo::win_tcp`。
//!
//! 编译范围：`cfg(any(windows, test))`。macOS 正式构建不包含这些代码。

use std::fmt;
use std::net::{IpAddr, SocketAddr};

/// Win32 `ERROR_ACCESS_DENIED`。
pub const ERROR_ACCESS_DENIED: u32 = 5;

/// 生产环境的 Verge 内核映像白名单（§2.2 L2）。只比较文件名，大小写不敏感。
pub const VERGE_CORE_IMAGES: &[&str] = &["verge-mihomo.exe", "verge-mihomo-alpha.exe"];

/// S-2 诊断里视为「宽泛主体」的 SID：BU（Users）、AU（Authenticated Users）、WD（Everyone）。
const BROAD_TRUSTEES: &[(&str, &str)] = &[
    ("S-1-5-32-545", "BU"),
    ("S-1-5-11", "AU"),
    ("S-1-1-0", "WD"),
];

// ---- 访问掩码（winnt.h） -------------------------------------------------------------------
const FILE_WRITE_DATA: u32 = 0x0002;
/// 对命名管道而言 `FILE_APPEND_DATA` 即 `FILE_CREATE_PIPE_INSTANCE`。
const FILE_CREATE_PIPE_INSTANCE: u32 = 0x0004;
const WRITE_DAC: u32 = 0x0004_0000;
const WRITE_OWNER: u32 = 0x0008_0000;
const GENERIC_ALL: u32 = 0x1000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
/// `FILE_ALL_ACCESS` = STANDARD_RIGHTS_REQUIRED | SYNCHRONIZE | 0x1FF。
const FILE_ALL_ACCESS: u32 = 0x001F_01FF;
/// `FILE_GENERIC_WRITE` = READ_CONTROL | FILE_WRITE_DATA | FILE_WRITE_ATTRIBUTES | FILE_WRITE_EA
/// | FILE_APPEND_DATA | SYNCHRONIZE。
const FILE_GENERIC_WRITE: u32 = 0x0012_0116;
const MAXIMUM_ALLOWED: u32 = 0x0200_0000;
/// F3′（Ellie V5 复跑复核 §2-1）：外人的 allow 类 ACE 命中其中任一位即拒绝。原始掩码与
/// 映射后的掩码都检查（GENERIC_WRITE / GENERIC_ALL 映射后本来就含 0x4，这里显式列出便于审阅）。
const INSTANCE_OR_SD_WRITE: u32 = FILE_CREATE_PIPE_INSTANCE
    | WRITE_DAC
    | WRITE_OWNER
    | GENERIC_WRITE
    | GENERIC_ALL
    | MAXIMUM_ALLOWED;
/// 强制完整性标签 RID：Medium（SECURITY_MANDATORY_MEDIUM_RID）。
pub const MANDATORY_MEDIUM_RID: u32 = 0x2000;
/// S-2：诊断用的写位（映射后）。
const BROAD_WRITE_BITS: u32 = FILE_WRITE_DATA | FILE_CREATE_PIPE_INSTANCE | WRITE_DAC | WRITE_OWNER;

/// 一个 SID 相对于本进程的身份类别。`sid` 只用于日志 / 诊断，判定只看 `who`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Who {
    /// 与本进程 TokenUser 相同。
    Me,
    /// LocalSystem（S-1-5-18）。
    System,
    /// BUILTIN\Administrators（S-1-5-32-544）。
    Admins,
    /// 其他任何主体。
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ident {
    pub who: Who,
    pub sid: String,
}

impl Ident {
    pub fn new(who: Who, sid: impl Into<String>) -> Self {
        Ident {
            who,
            sid: sid.into(),
        }
    }
}

/// 一次失败的 Win32 / NT 调用：哪个 API、Win32 错误码。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FactErr {
    pub api: &'static str,
    pub code: u32,
}

impl FactErr {
    pub fn new(api: &'static str, code: u32) -> Self {
        FactErr { api, code }
    }
    pub fn is_access_denied(&self) -> bool {
        self.code == ERROR_ACCESS_DENIED
    }
}

impl fmt::Display for FactErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} 失败（Win32 {}）", self.api, self.code)
    }
}

/// 命名管道的用途类别，由 `guard::check_pipe_name` 从名字推出（§2.2 L0）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipeKind {
    /// `verge-mihomo-sidecar-{release,dev}[-key]`：Verge 以当前用户身份拉起的内核。
    Sidecar,
    /// `verge-mihomo-{production,development}[-key]`：clash-verge-service（SYSTEM）拉起的内核。
    Service,
    /// 不带后缀的 `verge-mihomo`（旧版 Verge；仅在 yaml 显式写出时使用，N-2）。
    Legacy,
}

/// L1：管道属主是否属于该类别的期望集合（§2.2 L1 真值表）。
pub fn owner_allowed(kind: PipeKind, owner: Who) -> bool {
    match kind {
        PipeKind::Sidecar => matches!(owner, Who::Me | Who::Admins),
        PipeKind::Service => matches!(owner, Who::System | Who::Admins),
        PipeKind::Legacy => matches!(owner, Who::Me | Who::System | Who::Admins),
    }
}

// ---- DACL ----------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AceKind {
    /// ACCESS_ALLOWED_ACE_TYPE（0）。
    Allow,
    /// ACCESS_DENIED_ACE_TYPE（1）。
    Deny,
    /// 其他「允许」类 ACE（对象 / 回调 ACE 等），掩码与主体未逐一解析。
    OtherAllow,
    /// 审计 / 其他不授予访问的 ACE。
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AceFact {
    pub kind: AceKind,
    pub mask: u32,
    /// INHERIT_ONLY_ACE：只用于继承，不作用于管道本身。
    pub inherit_only: bool,
    /// None：ACE 里的 SID 无法解析。
    pub trustee: Option<Ident>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DaclFact {
    /// NULL DACL 或 DACL 不存在（present = false）：任何人都有全部权限。拒绝。
    Null,
    /// 逐条 ACE。空列表（ACE 数为 0）= 只有属主的隐式权利：允许（属主已由 L1 限定）。
    Aces(Vec<AceFact>),
}

/// SACL 里的强制完整性标签。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LabelFact {
    /// 没有显式标签（等同 Medium）。
    None,
    /// `S-1-16-<rid>` 的 RID（Low = 0x1000，Medium = 0x2000，High = 0x3000，System = 0x4000），
    /// 且每条标签 ACE 都带 `SYSTEM_MANDATORY_LABEL_NO_WRITE_UP`（0x1）。
    Rid(u32),
    /// 有标签 ACE 不带 NO_WRITE_UP（如 `S:(ML;;NR;;;ME)`）：低完整性进程仍可写入 / 插入实例。
    /// 拒绝（Ellie PR50 Q4）。值为最低 RID。
    WithoutNoWriteUp(u32),
    /// 有标签 ACE 但解析不了。
    Unparsed,
}

/// 一次 `GetSecurityInfo(OWNER | DACL | LABEL)` 的结果（Ellie §2-3：同一句柄、一次调用）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecurityFact {
    pub owner: Ident,
    pub dacl: DaclFact,
    pub label: LabelFact,
}

/// 按文件对象的通用映射展开 GENERIC_* 位（F3：要看映射后的位，而不只是 GENERIC_WRITE）。
pub fn map_generic(mask: u32) -> u32 {
    let mut m = mask;
    if mask & GENERIC_ALL != 0 {
        m |= FILE_ALL_ACCESS;
    }
    if mask & GENERIC_WRITE != 0 {
        m |= FILE_GENERIC_WRITE;
    }
    m
}

fn trusted_trustee(t: &Option<Ident>) -> bool {
    matches!(
        t.as_ref().map(|i| i.who),
        Some(Who::Me | Who::System | Who::Admins)
    )
}

/// F3′ 的 DACL 判定结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DaclVerdict {
    Ok,
    /// NULL / 不存在的 DACL。
    Null,
    /// 有不认识的 ACE 类型（object / callback / conditional / compound 等）。
    UnknownAce,
    /// 外人（本人 / SYSTEM / BA 之外的任何 SID）的 allow ACE 命中 [`INSTANCE_OR_SD_WRITE`]。
    ForeignInstance(String),
}

/// F3′（V5 复跑 S-D′ + Ellie 复核 §2-1/§2-2）：**白名单**判定管道 DACL。
///
/// V5 复跑实测：mihomo 默认 SDDL 下另一个用户成功插入了第二个实例；客户端在**同一个句柄**上
/// 读到的 GetNamedPipeServerProcessId、属主、L3 都指向 mihomo / 本人，HTTP 回应却来自他人的
/// 实例。属主与 DACL 是管道级（FCB）的、所有实例共享，所以唯一可靠的保证是「DACL 不允许
/// 外人创建实例或改写安全描述符」。
///
/// 规则：受托人只信任 {本人, S-1-5-18, S-1-5-32-544}；其余 SID（WD / AU / BU / IU、logon SID、
/// AppContainer、RESTRICTED、CREATOR OWNER、OWNER RIGHTS、解析不了的 SID…）一律是外人；
/// deny ACE 一律忽略（不做顺序抵消，只会更严）；INHERIT_ONLY 的 ACE 照样计入；不认识的
/// ACE 类型直接拒绝；NULL DACL 拒绝；空 DACL 通过。
pub fn dacl_verdict(d: &DaclFact) -> DaclVerdict {
    let aces = match d {
        DaclFact::Null => return DaclVerdict::Null,
        DaclFact::Aces(a) => a,
    };
    let mut hits = Vec::new();
    for a in aces {
        match a.kind {
            AceKind::Deny => {}
            AceKind::OtherAllow | AceKind::Other => return DaclVerdict::UnknownAce,
            AceKind::Allow => {
                let bits = (a.mask | map_generic(a.mask)) & INSTANCE_OR_SD_WRITE;
                if !trusted_trustee(&a.trustee) && bits != 0 {
                    let who = a
                        .trustee
                        .as_ref()
                        .map(|t| t.sid.as_str())
                        .unwrap_or("<无法解析的 SID>");
                    hits.push(format!("{who}=0x{:08X}", a.mask));
                }
            }
        }
    }
    if hits.is_empty() {
        DaclVerdict::Ok
    } else {
        DaclVerdict::ForeignInstance(hits.join(", "))
    }
}

/// S-2：BU / AU / WD 带写位时给出诊断提示（不拒绝）。返回 `None` 表示没有宽泛写权限。
pub fn dacl_broad_write_hint(d: &DaclFact) -> Option<String> {
    match d {
        DaclFact::Null => Some("管道 DACL 为空（NULL DACL），任何人都有全部权限".into()),
        DaclFact::Aces(aces) => {
            let mut hits: Vec<String> = Vec::new();
            for a in aces {
                if a.inherit_only || a.kind != AceKind::Allow {
                    continue;
                }
                let Some(t) = &a.trustee else { continue };
                let Some((_, alias)) = BROAD_TRUSTEES.iter().find(|(sid, _)| *sid == t.sid) else {
                    continue;
                };
                let bits = map_generic(a.mask) & BROAD_WRITE_BITS;
                if bits != 0 {
                    hits.push(format!("{alias}=0x{:08X}", a.mask));
                }
            }
            if hits.is_empty() {
                None
            } else {
                Some(format!(
                    "管道 DACL 授予宽泛主体写权限（{}），同机其他用户可写入或插入实例",
                    hits.join(", ")
                ))
            }
        }
    }
}

// ---- 管道判定（§2.2） ----------------------------------------------------------------------

/// 一次已打开管道连接上的 OS 事实。实现方必须针对**同一个句柄**采集（L1 与连接绑定），
/// 并在 L2 ① 与 L3 之间持有同一个进程句柄（Ellie S1）。
///
/// 信任只来自 [`PipeFacts::security`]；服务端 PID / L2 / L3 在管道路径上**只作诊断**
/// （V5 S-D′：它们不能标识实际连上的实例）。
pub trait PipeFacts {
    /// L1 + F3′ + 标签：一次 `GetSecurityInfo(OWNER | DACL | LABEL)`。
    fn security(&mut self) -> Result<SecurityFact, FactErr>;
    /// `GetNamedPipeServerProcessId`（诊断）。
    fn server_pid(&mut self) -> Result<u32, FactErr>;
    /// L2 ①：`OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `QueryFullProcessImageNameW`（Win32 路径）。
    fn image_win32_path(&mut self, pid: u32) -> Result<String, FactErr>;
    /// L2 ②：`NtQuerySystemInformation(SystemProcessIdInformation)`（NT 路径）。
    fn image_nt_path(&mut self, pid: u32) -> Result<String, FactErr>;
    /// L2 ③：Toolhelp32 `szExeFile`（只有文件名）。
    fn image_snapshot_name(&mut self, pid: u32) -> Result<String, FactErr>;
    /// L3：进程令牌的 TokenUser。
    fn token_user(&mut self, pid: u32) -> Result<Ident, FactErr>;
    /// 服务模式内核是否位于 `%ProgramData%\clash-verge-service\cores\` 下（`nt` 表示 NT 路径）。
    /// None：无法判断。只用于告警。
    fn under_service_cores_dir(&mut self, path: &str, nt: bool) -> Option<bool>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum L2Via {
    /// ① QueryFullProcessImageNameW
    Win32Path,
    /// ② NtQuerySystemInformation
    NtPath,
    /// ③ Toolhelp32
    Snapshot,
}

/// 校验通过的管道连接。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PipeTrust {
    pub kind: PipeKind,
    pub owner: Ident,
    pub label: LabelFact,
    /// 以下三项只作诊断（不参与信任判定）。
    pub server_pid: Option<u32>,
    pub image: Option<(String, L2Via)>,
    pub token: Option<Ident>,
    /// 只告警、不拒绝的诊断（S-2、L2 / L3 不一致、服务模式路径）。
    pub warnings: Vec<String>,
    /// S-2：DACL 给 BU / AU / WD 授予了写位（[`dacl_broad_write_hint`] 命中）。走到这里说明 F3′
    /// 已通过，所以这些宽泛主体**不能**创建实例或改写安全描述符（不能冒充），但能以读写方式
    /// 连上管道、向控制器发请求（如 cvsi 风格 `AU=0x0012019B`）。来源标签里记为
    /// `;pipe-open-to-all=<name>`：只提示，不拒绝。
    pub open_to_all: bool,
}

/// 管道校验失败的内容（`PipeErr::Untrusted` / `PipeErr::InsecureDacl`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Untrusted {
    /// 机器可读的原因码，如 `owner-mismatch`、`dacl-allows-foreign-instance`。
    pub reason: &'static str,
    pub detail: String,
    pub owner_sid: Option<String>,
    pub server_pid: Option<u32>,
}

impl fmt::Display for Untrusted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}：{}", self.reason, self.detail)?;
        if let Some(sid) = &self.owner_sid {
            write!(f, "（属主 {sid}）")?;
        }
        if let Some(pid) = self.server_pid {
            write!(f, "（服务端 PID {pid}）")?;
        }
        Ok(())
    }
}

/// 文件名部分（同时处理 `\`、`/` 和 NT 路径）。
pub fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// 映像文件名是否在白名单中（大小写不敏感）。
pub fn image_allowed(images: &[String], path_or_name: &str) -> bool {
    let name = file_name(path_or_name);
    images.iter().any(|i| i.eq_ignore_ascii_case(name))
}

/// 属于「安全描述符配置不安全」（`PipeErr::InsecureDacl`）的原因码：属主可信，但 DACL / 标签
/// 允许外人插入实例。其余原因码属于 `PipeErr::Untrusted`。
pub const INSECURE_DACL_REASONS: &[&str] = &[
    "dacl-allows-foreign-instance",
    "dacl-null",
    "dacl-unknown-ace",
    "label-below-medium",
    "label-unparsed",
    "label-no-write-up",
];

/// 标签 ACE 的 `SYSTEM_MANDATORY_LABEL_NO_WRITE_UP`。
pub const MANDATORY_NO_WRITE_UP: u32 = 0x1;

/// §2.2 管道判定。信任 = L1 属主（按 PipeKind）+ F3′ DACL 白名单 + 完整性标签；三者来自同一句柄上
/// 的一次 `GetSecurityInfo`。服务端 PID、L2、L3 只作诊断。调用方保证判定通过之前不向管道写任何
/// 字节（F5）。
///
/// **接受的风险 R3（WRITE_DAC 注入后复原，TOCTOU）**：安全描述符只在连接后读取一次。持有
/// `WRITE_DAC`（或 `WRITE_OWNER`）的主体可以先放宽 DACL、插入自己的实例、再把 DACL 改回原样；
/// 这样的实例在读取时看起来是安全的，本函数无法发现。之所以接受：在 mihomo 默认 SDDL 与 Verge
/// SDDL 下，`WRITE_DAC` / `WRITE_OWNER` 只授予受信主体（本人 / SYSTEM / BA，或经 GENERIC_ALL
/// 映射），而它们本来就能直接冒充控制器；任何把这些位授予其他主体的 DACL 都会在下面的 F3′
/// 白名单里被拒绝。详见 docs/windows-naming.md「接受的风险」。
pub fn decide_pipe(
    kind: PipeKind,
    images: &[String],
    facts: &mut dyn PipeFacts,
) -> Result<PipeTrust, Untrusted> {
    let reject = |reason, detail: String, owner: Option<&Ident>| Untrusted {
        reason,
        detail,
        owner_sid: owner.map(|o| o.sid.clone()),
        server_pid: None,
    };

    let sec = facts.security().map_err(|e| {
        reject(
            "security-unreadable",
            format!("读不到管道的属主 / DACL / 标签：{e}"),
            None,
        )
    })?;
    let owner = sec.owner;

    // L1：属主
    if !owner_allowed(kind, owner.who) {
        return Err(reject(
            "owner-mismatch",
            format!(
                "{kind:?} 管道的属主应为期望集合之一，实际为 {:?}",
                owner.who
            ),
            Some(&owner),
        ));
    }

    // F3′：DACL 白名单（强制）。R3（WRITE_DAC 注入后复原）是接受的风险，见上方函数注释与
    // docs/windows-naming.md：这里只能看到读取时刻的 DACL。
    const STALE: &str = "；若 Verge 已升级仍出现，可能是同名管道已被占用（旧实例未退出）";
    match dacl_verdict(&sec.dacl) {
        DaclVerdict::Ok => {}
        DaclVerdict::Null => {
            return Err(reject(
                "dacl-null",
                format!("管道 DACL 为 NULL 或不存在，任何人都能插入实例{STALE}"),
                Some(&owner),
            ))
        }
        DaclVerdict::UnknownAce => {
            return Err(reject(
                "dacl-unknown-ace",
                "管道 DACL 含有无法判定的 ACE 类型".to_string(),
                Some(&owner),
            ))
        }
        DaclVerdict::ForeignInstance(hits) => {
            return Err(reject(
                "dacl-allows-foreign-instance",
                format!("管道 DACL 允许其他主体创建实例或改写安全描述符（{hits}）{STALE}"),
                Some(&owner),
            ))
        }
    }

    // 完整性标签：低于 Medium 时同用户的低完整性进程也能插入实例（Ellie §2-7）。
    match sec.label {
        LabelFact::None => {}
        LabelFact::Rid(r) if r >= MANDATORY_MEDIUM_RID => {}
        LabelFact::Rid(r) => {
            return Err(reject(
                "label-below-medium",
                format!("管道完整性标签 0x{r:X} 低于 Medium"),
                Some(&owner),
            ))
        }
        LabelFact::WithoutNoWriteUp(r) => {
            return Err(reject(
                "label-no-write-up",
                format!("管道完整性标签（0x{r:X}）不含 NO_WRITE_UP，低完整性进程仍可写入"),
                Some(&owner),
            ))
        }
        LabelFact::Unparsed => {
            return Err(reject(
                "label-unparsed",
                "管道的完整性标签无法解析".to_string(),
                Some(&owner),
            ))
        }
    }

    let mut warnings = Vec::new();
    // NULL DACL 也会命中 S-2，但它已在上面被 F3′ 拒绝，到这里只剩 ACE 列表的情形。
    let open_to_all = match dacl_broad_write_hint(&sec.dacl) {
        Some(h) => {
            warnings.push(h);
            true
        }
        None => false,
    };

    // ---- 以下只作诊断（V5 S-D′：PID / L2 / L3 不能标识连上的实例） ----
    let server_pid = match facts.server_pid() {
        Ok(p) => Some(p),
        Err(e) => {
            warnings.push(format!("诊断：读不到服务端 PID：{e}"));
            None
        }
    };
    let mut image = None;
    let mut token = None;
    if let Some(pid) = server_pid {
        image = match facts.image_win32_path(pid) {
            Ok(p) => Some((p, L2Via::Win32Path)),
            Err(e1) => match facts.image_nt_path(pid) {
                Ok(p) => Some((p, L2Via::NtPath)),
                Err(e2) => match facts.image_snapshot_name(pid) {
                    Ok(p) => Some((p, L2Via::Snapshot)),
                    Err(e3) => {
                        warnings.push(format!("诊断：L2 ①②③ 均失败：{e1}；{e2}；{e3}"));
                        None
                    }
                },
            },
        };
        if let Some((p, _)) = &image {
            if !image_allowed(images, p) {
                warnings.push(format!(
                    "诊断：服务端映像 {} 不在白名单 {images:?} 中",
                    file_name(p)
                ));
            }
        }
        match facts.token_user(pid) {
            Ok(t) => {
                let expected = match owner.who {
                    Who::Me => t.who == Who::Me,
                    _ => matches!(t.who, Who::Me | Who::System),
                };
                if !expected {
                    warnings.push(format!("诊断：服务端令牌用户为 {}（{:?}）", t.sid, t.who));
                }
                token = Some(t);
            }
            Err(e) => {
                // 属主为本人时 ACCESS_DENIED 是常态（F3），不提示。
                if owner.who == Who::Me && !e.is_access_denied() {
                    warnings.push(format!("诊断：读不到服务端令牌：{e}"));
                }
            }
        }
        // 服务模式路径告警（只告警）。
        if let Some((path, via)) = &image {
            if owner.who == Who::System
                && *via != L2Via::Snapshot
                && facts.under_service_cores_dir(path, *via == L2Via::NtPath) == Some(false)
            {
                warnings.push(format!(
                    "服务模式内核不在 %ProgramData%\\clash-verge-service\\cores\\ 下：{path}"
                ));
            }
        }
    }

    Ok(PipeTrust {
        kind,
        owner,
        label: sec.label,
        server_pid,
        image,
        token,
        warnings,
        open_to_all,
    })
}

// ---- 管道错误与 M-3 回落规则 ----------------------------------------------------------------

/// 打开 / 校验 / 请求命名管道的结果分类（§2.2）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PipeErr {
    /// ERROR_FILE_NOT_FOUND：这个名字下没有服务端。
    NotFound,
    /// ERROR_PIPE_BUSY 直到截止时间。
    Busy,
    /// 打开时 ERROR_ACCESS_DENIED（DACL 不允许本人连接）。
    Denied,
    /// 连上了，但属主 / 安全描述符不可读等信任链失败。零字节（F5）。停止发现，不回落 TCP（M-3）。
    Untrusted(Untrusted),
    /// 属主可信，但 DACL / 标签允许外人插入实例（F3′）。零字节；TCP 只在 listener-verified 时可用。
    InsecureDacl(Untrusted),
    /// 其他 I/O 错误（含校验通过后的读写失败）。
    Io(String),
}

impl fmt::Display for PipeErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PipeErr::NotFound => f.write_str("命名管道不存在"),
            PipeErr::Busy => f.write_str("命名管道忙（等待超时）"),
            PipeErr::Denied => f.write_str("无权连接命名管道"),
            PipeErr::Untrusted(u) => write!(f, "命名管道未通过校验：{u}"),
            PipeErr::InsecureDacl(u) => write!(
                f,
                "{PIPE_INSECURE_DACL_UI}（{u}）建议：{PIPE_INSECURE_DACL_ACTION}"
            ),
            PipeErr::Io(e) => write!(f, "命名管道 I/O 错误：{e}"),
        }
    }
}

/// F3′ 拒绝时给用户看的文案（Ellie 复核 §4）。
pub const PIPE_INSECURE_DACL_UI: &str =
    "已拒绝连接：该 Verge 内核的控制管道允许本机其他用户冒充，存在被劫持风险。";
/// 对应的操作按钮文案（前端按钮在 M3 接入）。
pub const PIPE_INSECURE_DACL_ACTION: &str = "升级 Clash Verge 并重启内核";

impl PipeErr {
    /// 把判定结果映射为 Untrusted / InsecureDacl。
    pub fn from_reject(u: Untrusted) -> Self {
        if INSECURE_DACL_REASONS.contains(&u.reason) {
            PipeErr::InsecureDacl(u)
        } else {
            PipeErr::Untrusted(u)
        }
    }
}

/// M-3：管道候选全部失败后，TCP 能否作为回落。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TcpFallback {
    /// 所有候选均为 NotFound / Busy / Denied。`denied_seen`：其中至少一个 Denied
    /// （若将来放开未验证 TCP 的只读访问，Denied 时也必须拒绝，§2.1 规则 4）。
    Allowed { denied_seen: bool },
    /// 有候选是 InsecureDacl：TCP 只在 listener-verified 时可用（Ellie 复核 §2-10）。
    VerifiedOnly(String),
    /// 有候选被判为不可信（或出现其他错误）：停止，不回落 TCP。
    Blocked(String),
}

/// 严格程度：Allowed < VerifiedOnly < Blocked。
fn fallback_rank(f: &TcpFallback) -> u8 {
    match f {
        TcpFallback::Allowed { .. } => 0,
        TcpFallback::VerifiedOnly(_) => 1,
        TcpFallback::Blocked(_) => 2,
    }
}

/// Ellie PR50 M-a：**单次**管道请求的结果只能收紧 M-3 状态，不能放宽。
///
/// `current` 为当前有效（未过期）的缓存。返回 `Some(new)` 表示应写入（并刷新时间戳）；
/// `None` 表示保持原状。成功、NotFound、Busy（= Allowed）永远不写：放宽只能由一轮完整的
/// 候选探测（`record_probe_outcome`）做出；缓存为空时也不写 Allowed，以免跳过下一次完整探测。
pub fn tighten_fallback(current: Option<&TcpFallback>, new: TcpFallback) -> Option<TcpFallback> {
    let cur = current.map(fallback_rank).unwrap_or(0);
    (fallback_rank(&new) > cur).then_some(new)
}

/// 单次管道请求失败发生在哪一步（Ellie PR50 r2 非阻塞 #1）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipeStage {
    /// 还没碰管道（请求本身不合法）：与管道是否可信无关，不影响 M-3 状态。
    BeforeOpen,
    /// 打开 / 信任判定阶段（含打开时的未知 I/O 错误）：沿用 `tcp_fallback_after_pipes`。
    Opening,
    /// 管道已通过 `decide_pipe`（可信）之后的读写失败或超时。
    AfterTrust,
}

/// 单次管道请求失败后**应收紧到**的 M-3 状态（再经 `tighten_fallback` 只升不降）。
///
/// 可信管道上的一次 Io / 超时只说明这次请求没成功，不说明有人冒充：只收紧到
/// `VerifiedOnly`（TCP 仍可用，但仅限 listener-verified），而不是 `Blocked`——否则 TTL 内会
/// 跳过完整探测，TCP 回落被封 30 秒（可用性问题）。Untrusted 仍 `Blocked`，InsecureDacl 仍
/// `VerifiedOnly`；信任判定之前的 Io 无法区分「坏管道」与「临时故障」，仍按 `Blocked` 处理。
pub fn tcp_fallback_after_single_request(e: &PipeErr, stage: PipeStage) -> Option<TcpFallback> {
    match (stage, e) {
        (PipeStage::BeforeOpen, _) => None,
        (PipeStage::AfterTrust, PipeErr::Io(why)) => Some(TcpFallback::VerifiedOnly(format!(
            "可信命名管道上的单次请求失败（I/O / 超时），TCP 暂时仅限已验证的监听者：{why}"
        ))),
        _ => Some(tcp_fallback_after_pipes(std::slice::from_ref(e))),
    }
}

pub fn tcp_fallback_after_pipes(errs: &[PipeErr]) -> TcpFallback {
    let mut denied_seen = false;
    let mut insecure = None;
    for e in errs {
        match e {
            PipeErr::NotFound | PipeErr::Busy => {}
            PipeErr::Denied => denied_seen = true,
            PipeErr::InsecureDacl(u) => {
                insecure.get_or_insert_with(|| {
                    format!("命名管道安全描述符不安全，TCP 仅限已验证的监听者：{u}")
                });
            }
            PipeErr::Untrusted(u) => {
                return TcpFallback::Blocked(format!("命名管道未通过校验，不回落 TCP：{u}"))
            }
            PipeErr::Io(e) => {
                return TcpFallback::Blocked(format!("命名管道 I/O 错误，不回落 TCP：{e}"))
            }
        }
    }
    match insecure {
        Some(why) => TcpFallback::VerifiedOnly(why),
        None => TcpFallback::Allowed { denied_seen },
    }
}

// ---- TCP 监听者校验（§2.1 规则 4，C-1 / C-4 / F4） -------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListenerRow {
    pub ip: IpAddr,
    pub pid: u32,
}

/// 某 PID 在 `WTSEnumerateProcessesExW(level 1)` 中的用户 SID（只作快速拒绝，C-4 / F4）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WtsUser {
    User(Ident),
    /// pUserSid 为 NULL（V5 S-C：SYSTEM 进程即如此）。
    NullSid,
    /// pUserSid 非空但复制失败。
    SidCopyFailed,
    /// 快照中没有这个 PID。
    NotInSnapshot,
    /// WTS 调用本身失败。
    Failed(FactErr),
}

/// 监听进程身份事实（L2 ①②③、L3、WTS）。实现方在 ① 与 L3 之间持有同一个进程句柄（Ellie S1）。
pub trait ProcessFacts {
    fn image_win32_path(&mut self, pid: u32) -> Result<String, FactErr>;
    fn image_nt_path(&mut self, pid: u32) -> Result<String, FactErr>;
    fn image_snapshot_name(&mut self, pid: u32) -> Result<String, FactErr>;
    fn token_user(&mut self, pid: u32) -> Result<Ident, FactErr>;
    fn wts_user(&mut self, pid: u32) -> WtsUser;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TcpReject {
    /// 同一端口、同一地址族的回环 / 通配监听者不止一个进程（C-1）。
    ListenerConflict(Vec<u32>),
    /// 令牌可读但不是本人（也不是 SYSTEM）。
    TokenMismatch { pid: u32, sid: String },
    /// L2 ① 读到的映像不在白名单。
    ImageMismatch { pid: u32, image: String },
}

impl TcpReject {
    pub fn code(&self) -> &'static str {
        match self {
            TcpReject::ListenerConflict(_) => "listener-conflict",
            TcpReject::TokenMismatch { .. } => "token-mismatch",
            TcpReject::ImageMismatch { .. } => "image-mismatch",
        }
    }
}

impl fmt::Display for TcpReject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TcpReject::ListenerConflict(pids) => {
                write!(f, "listener-conflict：端口上有多个监听进程 {pids:?}")
            }
            TcpReject::TokenMismatch { pid, sid } => {
                write!(f, "token-mismatch：监听进程 {pid} 的令牌用户为 {sid}")
            }
            TcpReject::ImageMismatch { pid, image } => {
                write!(
                    f,
                    "image-mismatch：监听进程 {pid} 的映像 {image} 不在白名单"
                )
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListenerCheck {
    /// 唯一监听进程，L2 ① 在白名单，L3 读到本人，WTS 不是他人。
    Verified {
        pid: u32,
        image: String,
    },
    /// 无法证明是本人的内核（读不到、SYSTEM 服务模式、API 失败等）。`reasons` 只用于日志。
    Unverified {
        pid: Option<u32>,
        reasons: Vec<String>,
        service_mode: bool,
    },
    Rejected(TcpReject),
    /// 该地址族上没有回环 / 通配监听者。`ipv6_dual_stack_hint`：IPv4 地址无监听者，
    /// 但 IPv6 表上同端口有 `[::]` 监听（S-R12）。
    NotListening {
        ipv6_dual_stack_hint: bool,
    },
}

/// 某个地址族的监听表（已按端口过滤）；`Err` 表示 GetExtendedTcpTable 失败。
pub type ListenerTable = Result<Vec<ListenerRow>, FactErr>;

/// §2.1 规则 4。`v4` / `v6` 为端口 P 上的监听行（两族都传，用于 S-R12 提示）。
pub fn decide_tcp_listener(
    addr: SocketAddr,
    v4: &ListenerTable,
    v6: &ListenerTable,
    images: &[String],
    facts: &mut dyn ProcessFacts,
) -> ListenerCheck {
    let (same, other) = if addr.is_ipv4() { (v4, v6) } else { (v6, v4) };
    let rows = match same {
        Ok(r) => r,
        Err(e) => {
            return ListenerCheck::Unverified {
                pid: None,
                reasons: vec![format!("读不到监听表：{e}")],
                service_mode: false,
            }
        }
    };
    // 只看本地地址等于目标地址或为通配地址的行（127.0.0.1 优先于 0.0.0.0，::1 优先于 ::；
    // 优先级只影响诊断，判定看去重后的 PID 集合）。
    let mut relevant: Vec<&ListenerRow> = rows
        .iter()
        .filter(|r| r.ip == addr.ip() || r.ip.is_unspecified())
        .collect();
    relevant.sort_by_key(|r| r.ip.is_unspecified());
    let mut pids: Vec<u32> = relevant.iter().map(|r| r.pid).collect();
    pids.sort_unstable();
    pids.dedup();
    // Ellie PR50 Q1：目标是 IPv4 时，IPv6 表里 `::` 通配（可能是双栈套接字）且 PID 不同的监听者
    // 也算冲突（保守；连接后的 PID 绑定本来也会兜住）。
    if addr.is_ipv4() && !pids.is_empty() {
        if let Ok(rows6) = other {
            for r in rows6.iter().filter(|r| r.ip.is_unspecified()) {
                if !pids.contains(&r.pid) {
                    pids.push(r.pid);
                }
            }
            pids.sort_unstable();
        }
    }

    if pids.len() > 1 {
        return ListenerCheck::Rejected(TcpReject::ListenerConflict(pids));
    }
    let Some(&pid) = pids.first() else {
        let hint =
            addr.is_ipv4() && matches!(other, Ok(r) if r.iter().any(|x| x.ip.is_unspecified()));
        return ListenerCheck::NotListening {
            ipv6_dual_stack_hint: hint,
        };
    };

    let mut reasons = Vec::new();
    let mut service_mode = false;
    let mut wts_foreign = false;

    // WTS：只做快速拒绝；空 SID / 复制失败分开记录（F4）。
    match facts.wts_user(pid) {
        WtsUser::User(u) if u.who == Who::Me => {}
        WtsUser::User(u) if u.who == Who::System => service_mode = true,
        // F2：真实跨用户组合里 WTS 都是 null，这一支实际不会触发；只作诊断，以 L3 为准。
        // 与 L3 矛盾时宁可判为 unverified，但不再单独据此拒绝。
        WtsUser::User(u) => {
            wts_foreign = true;
            reasons.push(format!("wts: 其他用户 {}（仅诊断）", u.sid))
        }
        WtsUser::NullSid => reasons.push("wts: pUserSid 为空（未知）".to_string()),
        WtsUser::SidCopyFailed => reasons.push("wts: SID 复制失败（未知）".to_string()),
        WtsUser::NotInSnapshot => reasons.push("wts: PID 不在快照中".to_string()),
        WtsUser::Failed(e) => reasons.push(format!("wts: {e}")),
    }

    // L2 ①（②③ 只作诊断）
    let image = match facts.image_win32_path(pid) {
        Ok(p) => {
            if !image_allowed(images, &p) {
                return ListenerCheck::Rejected(TcpReject::ImageMismatch {
                    pid,
                    image: file_name(&p).to_string(),
                });
            }
            Some(p)
        }
        Err(e) => {
            reasons.push(format!("L2 ①：{e}"));
            match facts.image_nt_path(pid) {
                Ok(p) => reasons.push(format!("L2 ②（仅诊断）：{p}")),
                Err(e) => reasons.push(format!("L2 ②（仅诊断）：{e}")),
            }
            match facts.image_snapshot_name(pid) {
                Ok(p) => reasons.push(format!("L2 ③（仅诊断）：{p}")),
                Err(e) => reasons.push(format!("L2 ③（仅诊断）：{e}")),
            }
            None
        }
    };

    // L3
    let token_me = match facts.token_user(pid) {
        Ok(t) if t.who == Who::Me => true,
        Ok(t) if t.who == Who::System => {
            // C-4：SYSTEM 服务模式一律 unverified。
            service_mode = true;
            reasons.push("L3：SYSTEM（服务模式）".to_string());
            false
        }
        Ok(t) => return ListenerCheck::Rejected(TcpReject::TokenMismatch { pid, sid: t.sid }),
        Err(e) => {
            reasons.push(format!("L3：{e}"));
            false
        }
    };

    match image {
        // WTS 的「未知」（空 SID 等）不阻止验证：L3 才是权威（F4）。
        Some(image) if token_me && !service_mode && !wts_foreign => {
            ListenerCheck::Verified { pid, image }
        }
        _ => ListenerCheck::Unverified {
            pid: Some(pid),
            reasons,
            service_mode,
        },
    }
}

/// Ellie S1：连接建立后，按本端 / 对端地址在连接表里找到服务端那一行，其 PID 必须等于
/// 校验通过的 PID。`observed`：`Ok(None)` 表示表里没有找到对应行。
pub fn post_connect_check(
    expected_pid: u32,
    observed: Result<Option<u32>, FactErr>,
) -> Result<(), String> {
    match observed {
        Ok(Some(pid)) if pid == expected_pid => Ok(()),
        Ok(Some(pid)) => Err(format!(
            "连接后的服务端 PID {pid} 与校验通过的监听进程 {expected_pid} 不一致"
        )),
        Ok(None) => Err("连接后在连接表中找不到服务端一侧".to_string()),
        Err(e) => Err(format!("连接后读不到连接表：{e}")),
    }
}

// ---- TCP 发送计划 --------------------------------------------------------------------------

/// listener-unverified 时的策略（§2.1 规则 4 允许二选一）。M1 选择 **拒绝**，理由见 PR 描述：
/// 未验证的来源可能是同机他人的内核，前端在 M3 之前无法把数据标成「来源未验证」。
pub const UNVERIFIED_TCP_POLICY_REJECT: bool = true;
const _: () = assert!(
    UNVERIFIED_TCP_POLICY_REJECT,
    "tcp_send_plan 只实现了拒绝策略"
);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TcpPlan {
    /// 不建立连接（零字节）。
    Refuse(String),
    /// 连接并发送请求，不带 Authorization。
    SendWithoutAuth,
    /// 连接并发送带 `Authorization: Bearer` 的请求（仅 listener-verified 且闸门打开）。
    SendBearer,
}

/// 由监听者判定、是否有 secret、secret 闸门、M-3 回落状态决定这次 TCP 请求怎么发。
///
/// `secret_gate_open` 在生产中恒为 [`crate::mihomo::win_tcp::WINDOWS_TCP_SECRET_GATE_OPEN`]（false），
/// 直到 V5 复跑（S-F2 / S-D′）结论写入 PR（Ellie 时序条件 d）。
pub fn tcp_send_plan(
    check: &ListenerCheck,
    has_secret: bool,
    secret_gate_open: bool,
    fallback: &TcpFallback,
) -> TcpPlan {
    match fallback {
        TcpFallback::Blocked(why) => return TcpPlan::Refuse(why.clone()),
        TcpFallback::VerifiedOnly(why) if !matches!(check, ListenerCheck::Verified { .. }) => {
            return TcpPlan::Refuse(why.clone())
        }
        _ => {}
    }
    match check {
        ListenerCheck::Rejected(r) => TcpPlan::Refuse(format!("监听者校验失败：{r}")),
        ListenerCheck::NotListening {
            ipv6_dual_stack_hint,
        } => TcpPlan::Refuse(if *ipv6_dual_stack_hint {
            "控制器端口在 IPv4 回环上没有监听者（只有 [::] 监听；可尝试 [::1]）".to_string()
        } else {
            "控制器端口没有监听者".to_string()
        }),
        ListenerCheck::Unverified {
            reasons,
            service_mode,
            ..
        } => TcpPlan::Refuse(format!(
            "无法验证监听进程身份{}，不连接（listener-unverified）：{}",
            if *service_mode {
                "（服务模式）"
            } else {
                ""
            },
            reasons.join("；")
        )),
        ListenerCheck::Verified { .. } => {
            if !has_secret {
                TcpPlan::SendWithoutAuth
            } else if secret_gate_open {
                TcpPlan::SendBearer
            } else {
                TcpPlan::Refuse(
                    "Windows 上经 TCP 发送 secret 的通道暂未开放（等待 V5 复跑结论，M0 fail-closed）"
                        .to_string(),
                )
            }
        }
    }
}

// ---- TrustPolicy（F1：期望集合 / 白名单 / 可注入的 OS 事实） ---------------------------------

/// 信任策略：映像白名单，以及（仅测试构建）覆盖任意一项 OS 事实的注入点。
///
/// 生产代码只用 [`TrustPolicy::production`]；`overrides` 字段只在 `cfg(test)` 下存在，
/// 正式构建里没有任何注入入口。期望属主集合由 [`owner_allowed`] 按 `PipeKind` 固定。
#[derive(Clone, Debug)]
pub struct TrustPolicy {
    pub images: Vec<String>,
    #[cfg(test)]
    pub overrides: Overrides,
}

impl TrustPolicy {
    pub fn production() -> Self {
        TrustPolicy {
            images: VERGE_CORE_IMAGES.iter().map(|s| s.to_string()).collect(),
            #[cfg(test)]
            overrides: Overrides::default(),
        }
    }

    /// §2.2 判定（测试构建中先套上 overrides）。
    pub fn decide_pipe(
        &self,
        kind: PipeKind,
        facts: &mut dyn PipeFacts,
    ) -> Result<PipeTrust, Untrusted> {
        #[cfg(test)]
        {
            let mut o = Overlay {
                inner: facts,
                ov: &self.overrides,
            };
            decide_pipe(kind, &self.images, &mut o)
        }
        #[cfg(not(test))]
        {
            decide_pipe(kind, &self.images, facts)
        }
    }

    /// §2.1 规则 4 判定（测试构建中先套上 overrides）。
    pub fn decide_tcp(
        &self,
        addr: SocketAddr,
        v4: ListenerTable,
        v6: ListenerTable,
        facts: &mut dyn ProcessFacts,
    ) -> ListenerCheck {
        #[cfg(test)]
        {
            let ov = &self.overrides;
            let mut v4 = ov.listeners_v4.clone().unwrap_or(v4);
            if let Ok(rows) = v4.as_mut() {
                rows.extend(ov.extra_listeners_v4.iter().copied());
            }
            let v6 = ov.listeners_v6.clone().unwrap_or(v6);
            let mut o = Overlay { inner: facts, ov };
            decide_tcp_listener(addr, &v4, &v6, &self.images, &mut o)
        }
        #[cfg(not(test))]
        {
            decide_tcp_listener(addr, &v4, &v6, &self.images, facts)
        }
    }

    pub fn post_connect(
        &self,
        expected_pid: u32,
        observed: Result<Option<u32>, FactErr>,
    ) -> Result<(), String> {
        #[cfg(test)]
        let observed = self.overrides.conn_pid.unwrap_or(observed);
        post_connect_check(expected_pid, observed)
    }
}

/// 测试注入：`Some(..)` 的项替换真实事实。
#[cfg(test)]
#[derive(Clone, Debug, Default)]
pub struct Overrides {
    /// 让整次 GetSecurityInfo 失败。
    pub security_err: Option<FactErr>,
    pub owner: Option<Ident>,
    pub dacl: Option<DaclFact>,
    pub label: Option<LabelFact>,
    pub server_pid: Option<Result<u32, FactErr>>,
    pub l2_1: Option<Result<String, FactErr>>,
    pub l2_2: Option<Result<String, FactErr>>,
    pub l2_3: Option<Result<String, FactErr>>,
    pub l3: Option<Result<Ident, FactErr>>,
    pub wts: Option<WtsUser>,
    pub listeners_v4: Option<ListenerTable>,
    pub listeners_v6: Option<ListenerTable>,
    /// 追加到（真实或替换后的）IPv4 监听表，用于构造 listener-conflict。
    pub extra_listeners_v4: Vec<ListenerRow>,
    pub conn_pid: Option<Result<Option<u32>, FactErr>>,
    pub fallback: Option<TcpFallback>,
}

#[cfg(test)]
struct Overlay<'a, F: ?Sized> {
    inner: &'a mut F,
    ov: &'a Overrides,
}

#[cfg(test)]
impl<F: PipeFacts + ?Sized> PipeFacts for Overlay<'_, F> {
    fn security(&mut self) -> Result<SecurityFact, FactErr> {
        if let Some(e) = self.ov.security_err {
            return Err(e);
        }
        let mut s = self.inner.security()?;
        if let Some(o) = &self.ov.owner {
            s.owner = o.clone();
        }
        if let Some(d) = &self.ov.dacl {
            s.dacl = d.clone();
        }
        if let Some(l) = self.ov.label {
            s.label = l;
        }
        Ok(s)
    }
    fn server_pid(&mut self) -> Result<u32, FactErr> {
        self.ov
            .server_pid
            .unwrap_or_else(|| self.inner.server_pid())
    }
    fn image_win32_path(&mut self, pid: u32) -> Result<String, FactErr> {
        self.ov
            .l2_1
            .clone()
            .unwrap_or_else(|| self.inner.image_win32_path(pid))
    }
    fn image_nt_path(&mut self, pid: u32) -> Result<String, FactErr> {
        self.ov
            .l2_2
            .clone()
            .unwrap_or_else(|| self.inner.image_nt_path(pid))
    }
    fn image_snapshot_name(&mut self, pid: u32) -> Result<String, FactErr> {
        self.ov
            .l2_3
            .clone()
            .unwrap_or_else(|| self.inner.image_snapshot_name(pid))
    }
    fn token_user(&mut self, pid: u32) -> Result<Ident, FactErr> {
        self.ov
            .l3
            .clone()
            .unwrap_or_else(|| self.inner.token_user(pid))
    }
    fn under_service_cores_dir(&mut self, path: &str, nt: bool) -> Option<bool> {
        self.inner.under_service_cores_dir(path, nt)
    }
}

#[cfg(test)]
impl<F: ProcessFacts + ?Sized> ProcessFacts for Overlay<'_, F> {
    fn image_win32_path(&mut self, pid: u32) -> Result<String, FactErr> {
        self.ov
            .l2_1
            .clone()
            .unwrap_or_else(|| self.inner.image_win32_path(pid))
    }
    fn image_nt_path(&mut self, pid: u32) -> Result<String, FactErr> {
        self.ov
            .l2_2
            .clone()
            .unwrap_or_else(|| self.inner.image_nt_path(pid))
    }
    fn image_snapshot_name(&mut self, pid: u32) -> Result<String, FactErr> {
        self.ov
            .l2_3
            .clone()
            .unwrap_or_else(|| self.inner.image_snapshot_name(pid))
    }
    fn token_user(&mut self, pid: u32) -> Result<Ident, FactErr> {
        self.ov
            .l3
            .clone()
            .unwrap_or_else(|| self.inner.token_user(pid))
    }
    fn wts_user(&mut self, pid: u32) -> WtsUser {
        self.ov
            .wts
            .clone()
            .unwrap_or_else(|| self.inner.wts_user(pid))
    }
}

#[cfg(test)]
mod tests;
