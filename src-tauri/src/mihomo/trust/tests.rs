//! 纯判定的分支覆盖（F1）。每个用例都通过假事实注入一个分支，所有平台都会跑。

use super::*;
use std::collections::HashMap;

fn me() -> Ident {
    Ident::new(Who::Me, "S-1-5-21-1-2-3-1001")
}
fn system() -> Ident {
    Ident::new(Who::System, "S-1-5-18")
}
fn admins() -> Ident {
    Ident::new(Who::Admins, "S-1-5-32-544")
}
fn other() -> Ident {
    Ident::new(Who::Other, "S-1-5-21-1-2-3-1002")
}
fn denied(api: &'static str) -> FactErr {
    FactErr::new(api, ERROR_ACCESS_DENIED)
}
fn images() -> Vec<String> {
    VERGE_CORE_IMAGES.iter().map(|s| s.to_string()).collect()
}
fn ace(sid: &str, who: Who, mask: u32) -> AceFact {
    AceFact {
        kind: AceKind::Allow,
        mask,
        inherit_only: false,
        trustee: Some(Ident::new(who, sid)),
    }
}
/// Verge 服务 / sidecar 实测的 DACL：本人、SY、BA 全部权限（V5 S-A / S-C）。
fn verge_dacl() -> DaclFact {
    DaclFact::Aces(vec![
        ace("S-1-5-21-1-2-3-1001", Who::Me, GENERIC_ALL),
        ace("S-1-5-18", Who::System, GENERIC_ALL),
        ace("S-1-5-32-544", Who::Admins, GENERIC_ALL),
    ])
}
/// mihomo 默认 SDDL：`D:P(A;OICI;0x12019f;;;BU)(A;OICI;0x12019f;;;SY)`。
fn mihomo_default_dacl() -> DaclFact {
    DaclFact::Aces(vec![
        ace("S-1-5-32-545", Who::Other, 0x0012_019f),
        ace("S-1-5-18", Who::System, 0x0012_019f),
    ])
}

struct FakePipe {
    sec: Result<SecurityFact, FactErr>,
    pid: Result<u32, FactErr>,
    l2_1: Result<String, FactErr>,
    l2_2: Result<String, FactErr>,
    l2_3: Result<String, FactErr>,
    l3: Result<Ident, FactErr>,
    under_cores: Option<bool>,
    calls: Vec<&'static str>,
}

impl FakePipe {
    fn sidecar_ok() -> Self {
        FakePipe {
            sec: Ok(SecurityFact {
                owner: me(),
                dacl: verge_dacl(),
                label: LabelFact::None,
            }),
            pid: Ok(4242),
            l2_1: Ok(r"C:\Program Files\Clash Verge\verge-mihomo.exe".into()),
            l2_2: Ok(r"\Device\HarddiskVolume3\Program Files\Clash Verge\verge-mihomo.exe".into()),
            l2_3: Ok("verge-mihomo.exe".into()),
            l3: Ok(me()),
            under_cores: None,
            calls: vec![],
        }
    }
    /// V5 S-C 实测：SYSTEM 属主，① 与 L3 拒绝访问，② ③ 成功，WTS 为空。
    fn service_ok() -> Self {
        FakePipe {
            sec: Ok(SecurityFact {
                owner: system(),
                dacl: verge_dacl(),
                label: LabelFact::None,
            }),
            l2_1: Err(denied("OpenProcess")),
            l3: Err(denied("OpenProcess")),
            l2_2: Ok(
                r"\Device\HarddiskVolume3\ProgramData\clash-verge-service\cores\verge-mihomo.exe"
                    .into(),
            ),
            under_cores: Some(true),
            ..Self::sidecar_ok()
        }
    }
    fn with_owner(mut self, o: Ident) -> Self {
        self.sec.as_mut().unwrap().owner = o;
        self
    }
    fn with_dacl(mut self, d: DaclFact) -> Self {
        self.sec.as_mut().unwrap().dacl = d;
        self
    }
    fn with_label(mut self, l: LabelFact) -> Self {
        self.sec.as_mut().unwrap().label = l;
        self
    }
}

impl PipeFacts for FakePipe {
    fn security(&mut self) -> Result<SecurityFact, FactErr> {
        self.calls.push("security");
        self.sec.clone()
    }
    fn server_pid(&mut self) -> Result<u32, FactErr> {
        self.calls.push("pid");
        self.pid
    }
    fn image_win32_path(&mut self, _: u32) -> Result<String, FactErr> {
        self.calls.push("l2_1");
        self.l2_1.clone()
    }
    fn image_nt_path(&mut self, _: u32) -> Result<String, FactErr> {
        self.calls.push("l2_2");
        self.l2_2.clone()
    }
    fn image_snapshot_name(&mut self, _: u32) -> Result<String, FactErr> {
        self.calls.push("l2_3");
        self.l2_3.clone()
    }
    fn token_user(&mut self, _: u32) -> Result<Ident, FactErr> {
        self.calls.push("l3");
        self.l3.clone()
    }
    fn under_service_cores_dir(&mut self, _: &str, _: bool) -> Option<bool> {
        self.under_cores
    }
}

fn decide(kind: PipeKind, f: &mut FakePipe) -> Result<PipeTrust, Untrusted> {
    decide_pipe(kind, &images(), f)
}

#[test]
fn owner_allowed_truth_table() {
    use PipeKind::*;
    use Who::*;
    let table = [
        (Sidecar, [true, false, true, false]),
        (Service, [false, true, true, false]),
        (Legacy, [true, true, true, false]),
    ];
    for (kind, row) in table {
        for (who, want) in [Me, System, Admins, Other].into_iter().zip(row) {
            assert_eq!(owner_allowed(kind, who), want, "{kind:?} / {who:?}");
        }
    }
}

#[test]
fn sidecar_and_service_pass_with_diagnostics() {
    let mut f = FakePipe::sidecar_ok();
    let t = decide(PipeKind::Sidecar, &mut f).unwrap();
    assert_eq!(t.server_pid, Some(4242));
    assert_eq!(t.image.as_ref().map(|(_, v)| *v), Some(L2Via::Win32Path));
    assert_eq!(t.token, Some(me()));
    assert!(t.warnings.is_empty(), "{:?}", t.warnings);
    assert_eq!(f.calls[0], "security", "安全描述符必须最先读取");

    let mut f = FakePipe::service_ok();
    let t = decide(PipeKind::Service, &mut f).unwrap();
    assert_eq!(t.image.as_ref().map(|(_, v)| *v), Some(L2Via::NtPath));
    assert!(t.warnings.is_empty(), "{:?}", t.warnings);

    // BA 属主的 sidecar（管理员身份运行 Verge），BA 留在白名单（Ellie §2-9）
    let mut f = FakePipe::sidecar_ok().with_owner(admins());
    assert!(decide(PipeKind::Sidecar, &mut f).is_ok());
}

#[test]
fn security_unreadable_or_owner_outside_expected_set_rejects() {
    let mut f = FakePipe {
        sec: Err(FactErr::new("GetSecurityInfo", 5)),
        ..FakePipe::sidecar_ok()
    };
    let e = decide(PipeKind::Sidecar, &mut f).unwrap_err();
    assert_eq!(e.reason, "security-unreadable");
    assert!(matches!(PipeErr::from_reject(e), PipeErr::Untrusted(_)));
    assert_eq!(f.calls, vec!["security"], "读不到时不应继续采集");

    for (kind, owner) in [
        (PipeKind::Sidecar, system()),
        (PipeKind::Sidecar, other()),
        (PipeKind::Service, me()),
        (PipeKind::Service, other()),
        (PipeKind::Legacy, other()),
    ] {
        let mut f = FakePipe::sidecar_ok().with_owner(owner.clone());
        let e = decide(kind, &mut f).unwrap_err();
        assert_eq!(e.reason, "owner-mismatch", "{kind:?} {owner:?}");
        assert_eq!(e.owner_sid.as_deref(), Some(owner.sid.as_str()));
        assert!(matches!(PipeErr::from_reject(e), PipeErr::Untrusted(_)));
        assert_eq!(f.calls, vec!["security"]);
    }
}

/// V5 S-D′ / Ellie §8：管道路径上 PID / L2 / L3 只作诊断——失败或不一致都只告警，不拒绝。
#[test]
fn pid_l2_l3_are_diagnostics_only_on_pipe_path() {
    let cases: Vec<(&str, FakePipe)> = vec![
        (
            "PID 读不到",
            FakePipe {
                pid: Err(FactErr::new("GetNamedPipeServerProcessId", 1)),
                ..FakePipe::sidecar_ok()
            },
        ),
        (
            "映像不在白名单",
            FakePipe {
                l2_1: Ok(r"C:\Users\b\evil.exe".into()),
                ..FakePipe::sidecar_ok()
            },
        ),
        (
            "L2 全部失败",
            FakePipe {
                l2_1: Err(denied("OpenProcess")),
                l2_2: Err(FactErr::new("NtQuerySystemInformation", 5)),
                l2_3: Err(FactErr::new("Process32FirstW", 18)),
                ..FakePipe::sidecar_ok()
            },
        ),
        (
            "L3 为他人",
            FakePipe {
                l3: Ok(other()),
                ..FakePipe::sidecar_ok()
            },
        ),
        (
            "属主本人、L3 为 SYSTEM",
            FakePipe {
                l3: Ok(system()),
                ..FakePipe::sidecar_ok()
            },
        ),
        (
            "L3 读不到（非 ACCESS_DENIED）",
            FakePipe {
                l3: Err(FactErr::new("OpenProcessToken", 87)),
                ..FakePipe::sidecar_ok()
            },
        ),
        (
            "服务模式 L3 为他人",
            FakePipe {
                l3: Ok(other()),
                ..FakePipe::service_ok()
            },
        ),
    ];
    for (name, mut f) in cases {
        let kind = if f.sec.as_ref().unwrap().owner.who == Who::System {
            PipeKind::Service
        } else {
            PipeKind::Sidecar
        };
        let t = decide(kind, &mut f).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(
            t.warnings.iter().any(|w| w.starts_with("诊断")),
            "{name}: {:?}",
            t.warnings
        );
    }
    // 属主为本人时 L3 ACCESS_DENIED 是常态：不提示
    let mut f = FakePipe {
        l3: Err(denied("OpenProcess")),
        ..FakePipe::sidecar_ok()
    };
    assert!(decide(PipeKind::Sidecar, &mut f)
        .unwrap()
        .warnings
        .is_empty());
}

#[test]
fn image_whitelist_is_by_file_name_case_insensitive() {
    for ok in [
        r"C:\x\verge-mihomo.exe",
        r"C:\x\VERGE-MIHOMO-ALPHA.EXE",
        "verge-mihomo.exe",
    ] {
        assert!(image_allowed(&images(), ok), "{ok}");
    }
    for bad in [
        r"C:\x\verge-mihomo.exe.bak",
        r"C:\verge-mihomo.exe\evil.exe",
        "mihomo.exe",
        "",
    ] {
        assert!(!image_allowed(&images(), bad), "{bad}");
    }
}

/// Verge sidecar 的 SDDL `D:P(A;;GA;;;<本人>)(A;;GA;;;SY)(A;;GA;;;BA)`：通过 F3′。
#[test]
fn f3_prime_verge_sidecar_sddl_and_empty_dacl_pass() {
    assert_eq!(dacl_verdict(&verge_dacl()), DaclVerdict::Ok);
    let mut f = FakePipe::sidecar_ok();
    assert!(decide(PipeKind::Sidecar, &mut f).is_ok());
    let mut f = FakePipe::service_ok();
    assert!(decide(PipeKind::Service, &mut f).is_ok());
    // 空 DACL（0 条 ACE）：只有属主的隐式权利，属主已由 L1 限定（Ellie §2-2）
    assert_eq!(dacl_verdict(&DaclFact::Aces(vec![])), DaclVerdict::Ok);
    let mut f = FakePipe::sidecar_ok().with_dacl(DaclFact::Aces(vec![]));
    assert!(decide(PipeKind::Sidecar, &mut f).is_ok());
}

/// F3′（V5 复跑 S-D′ + Ellie 复核 §2-1）：外人的实例 / SD 写权限一律拒绝，而且是
/// InsecureDacl——不论 L3 / PID / 映像是否「正确」。
#[test]
fn f3_prime_foreign_instance_dacl_rejects_outright() {
    let foreign = "S-1-5-21-9-9-9-1002";
    let o = |sid: &str, mask: u32| ace(sid, Who::Other, mask);
    let risky: Vec<(&str, DaclFact, &str)> = vec![
        (
            "mihomo 默认 SDDL（BU 0x12019f 含 0x4）",
            mihomo_default_dacl(),
            "dacl-allows-foreign-instance",
        ),
        (
            "BU GENERIC_WRITE",
            DaclFact::Aces(vec![o("S-1-5-32-545", GENERIC_WRITE)]),
            "dacl-allows-foreign-instance",
        ),
        (
            "WD GENERIC_ALL",
            DaclFact::Aces(vec![o("S-1-1-0", GENERIC_ALL)]),
            "dacl-allows-foreign-instance",
        ),
        (
            "WD 0x4",
            DaclFact::Aces(vec![o("S-1-1-0", 0x4)]),
            "dacl-allows-foreign-instance",
        ),
        (
            "AU GENERIC_WRITE",
            DaclFact::Aces(vec![o("S-1-5-11", GENERIC_WRITE)]),
            "dacl-allows-foreign-instance",
        ),
        (
            "AU 0x12019f",
            DaclFact::Aces(vec![o("S-1-5-11", 0x0012_019f)]),
            "dacl-allows-foreign-instance",
        ),
        (
            "INTERACTIVE GENERIC_ALL",
            DaclFact::Aces(vec![o("S-1-5-4", GENERIC_ALL)]),
            "dacl-allows-foreign-instance",
        ),
        (
            "INTERACTIVE 0x4",
            DaclFact::Aces(vec![o("S-1-5-4", 0x4)]),
            "dacl-allows-foreign-instance",
        ),
        (
            "logon SID 0x4",
            DaclFact::Aces(vec![o("S-1-5-5-0-579189", 0x4)]),
            "dacl-allows-foreign-instance",
        ),
        (
            "AppContainer GW",
            DaclFact::Aces(vec![o("S-1-15-2-1", GENERIC_WRITE)]),
            "dacl-allows-foreign-instance",
        ),
        (
            "CREATOR OWNER GA",
            DaclFact::Aces(vec![o("S-1-3-0", GENERIC_ALL)]),
            "dacl-allows-foreign-instance",
        ),
        (
            "OWNER RIGHTS WRITE_DAC",
            DaclFact::Aces(vec![o("S-1-3-4", WRITE_DAC)]),
            "dacl-allows-foreign-instance",
        ),
        (
            "他人 WRITE_DAC",
            DaclFact::Aces(vec![o(foreign, WRITE_DAC)]),
            "dacl-allows-foreign-instance",
        ),
        (
            "他人 WRITE_OWNER",
            DaclFact::Aces(vec![o(foreign, WRITE_OWNER)]),
            "dacl-allows-foreign-instance",
        ),
        (
            "他人 MAXIMUM_ALLOWED",
            DaclFact::Aces(vec![o(foreign, MAXIMUM_ALLOWED)]),
            "dacl-allows-foreign-instance",
        ),
        ("NULL DACL", DaclFact::Null, "dacl-null"),
        (
            "仅继承的他人 GA（照样计入）",
            DaclFact::Aces(vec![AceFact {
                kind: AceKind::Allow,
                mask: GENERIC_ALL,
                inherit_only: true,
                trustee: Some(other()),
            }]),
            "dacl-allows-foreign-instance",
        ),
        (
            "SID 无法解析的允许 ACE",
            DaclFact::Aces(vec![AceFact {
                kind: AceKind::Allow,
                mask: 0x4,
                inherit_only: false,
                trustee: None,
            }]),
            "dacl-allows-foreign-instance",
        ),
        (
            "object / callback 允许 ACE（即使主体是本人）",
            DaclFact::Aces(vec![AceFact {
                kind: AceKind::OtherAllow,
                mask: 0,
                inherit_only: false,
                trustee: Some(me()),
            }]),
            "dacl-unknown-ace",
        ),
        (
            "不认识的 ACE 类型",
            DaclFact::Aces(vec![AceFact {
                kind: AceKind::Other,
                mask: 0,
                inherit_only: false,
                trustee: None,
            }]),
            "dacl-unknown-ace",
        ),
        (
            "Verge SDDL + 拒绝 ACE + BU 0x4（拒绝 ACE 不抵消）",
            {
                let DaclFact::Aces(mut v) = verge_dacl() else {
                    unreachable!()
                };
                v.insert(
                    0,
                    AceFact {
                        kind: AceKind::Deny,
                        mask: 0x4,
                        inherit_only: false,
                        trustee: Some(Ident::new(Who::Other, "S-1-5-32-545")),
                    },
                );
                v.push(o("S-1-5-32-545", 0x4));
                DaclFact::Aces(v)
            },
            "dacl-allows-foreign-instance",
        ),
    ];
    for (name, d, reason) in risky {
        assert_ne!(dacl_verdict(&d), DaclVerdict::Ok, "{name}");
        // L3 / 映像全部「正确」也拒绝；判定后不再采集任何诊断
        for (kind, base) in [
            (PipeKind::Sidecar, FakePipe::sidecar_ok()),
            (PipeKind::Service, FakePipe::service_ok()),
            (
                PipeKind::Legacy,
                FakePipe::sidecar_ok().with_owner(admins()),
            ),
        ] {
            let mut f = base.with_dacl(d.clone());
            let e = decide(kind, &mut f).unwrap_err();
            assert_eq!(e.reason, reason, "{name} / {kind:?}");
            assert_eq!(f.calls, vec!["security"], "{name}: DACL 拒绝后不应继续采集");
            assert!(
                matches!(PipeErr::from_reject(e), PipeErr::InsecureDacl(_)),
                "{name}"
            );
        }
    }

    // 不构成风险的 DACL：只读、拒绝 ACE、受信主体的全部权限、cvsi 风格 AU 0x12019b
    let safe = DaclFact::Aces(vec![
        o("S-1-5-32-545", 0x0012_0089), // FILE_GENERIC_READ
        o("S-1-1-0", 0x8000_0000),      // GENERIC_READ
        o("S-1-5-11", 0x0012_019b),     // cvsi：读写数据但不含 0x4
        AceFact {
            kind: AceKind::Deny,
            mask: GENERIC_ALL,
            inherit_only: false,
            trustee: Some(other()),
        },
        ace(
            "S-1-5-21-1-2-3-1001",
            Who::Me,
            GENERIC_ALL | MAXIMUM_ALLOWED,
        ),
        ace("S-1-5-18", Who::System, WRITE_DAC | WRITE_OWNER | 0x4),
        ace("S-1-5-32-544", Who::Admins, GENERIC_WRITE),
    ]);
    assert_eq!(dacl_verdict(&safe), DaclVerdict::Ok);
}

#[test]
fn integrity_label_below_medium_rejects() {
    for (l, ok) in [
        (LabelFact::None, true),
        (LabelFact::Rid(MANDATORY_MEDIUM_RID), true),
        (LabelFact::Rid(0x3000), true),
        (LabelFact::Rid(0x4000), true),
        (LabelFact::Rid(0x1000), false),
        (LabelFact::Rid(0), false),
        (LabelFact::Unparsed, false),
    ] {
        let mut f = FakePipe::sidecar_ok().with_label(l);
        let r = decide(PipeKind::Sidecar, &mut f);
        assert_eq!(r.is_ok(), ok, "{l:?}");
        if let Err(e) = r {
            assert!(matches!(PipeErr::from_reject(e), PipeErr::InsecureDacl(_)));
        }
    }
}

#[test]
fn insecure_dacl_ui_text() {
    let u = Untrusted {
        reason: "dacl-null",
        detail: "x".into(),
        owner_sid: None,
        server_pid: None,
    };
    let s = PipeErr::InsecureDacl(u).to_string();
    assert!(
        s.starts_with("已拒绝连接：该 Verge 内核的控制管道允许本机其他用户冒充，存在被劫持风险。"),
        "{s}"
    );
    assert!(s.contains("升级 Clash Verge 并重启内核"), "{s}");
}

#[test]
fn s2_broad_write_hint_is_warning_only() {
    assert!(dacl_broad_write_hint(&verge_dacl()).is_none());
    let h = dacl_broad_write_hint(&mihomo_default_dacl()).unwrap();
    assert!(h.contains("BU=0x0012019F"), "{h}");
    let wd = DaclFact::Aces(vec![ace("S-1-1-0", Who::Other, 0x0012_0089)]);
    assert!(dacl_broad_write_hint(&wd).is_none());
    // cvsi 风格 AU 0x12019b：不含 0x4，F3′ 通过，只给 S-2 提示（Ellie §2-4）
    let DaclFact::Aces(mut v) = verge_dacl() else {
        unreachable!()
    };
    v.push(ace("S-1-5-11", Who::Other, 0x0012_019b));
    let mut f = FakePipe::sidecar_ok().with_dacl(DaclFact::Aces(v));
    let t = decide(PipeKind::Sidecar, &mut f).unwrap();
    assert_eq!(t.warnings.len(), 1, "{:?}", t.warnings);
    assert!(t.warnings[0].contains("AU=0x0012019B"), "{:?}", t.warnings);
}

#[test]
fn service_core_outside_cores_dir_is_warning_only() {
    let mut f = FakePipe {
        under_cores: Some(false),
        ..FakePipe::service_ok()
    };
    let t = decide(PipeKind::Service, &mut f).unwrap();
    assert!(
        t.warnings.iter().any(|w| w.contains("cores")),
        "{:?}",
        t.warnings
    );
    let mut f = FakePipe {
        under_cores: None,
        ..FakePipe::service_ok()
    };
    assert!(decide(PipeKind::Service, &mut f)
        .unwrap()
        .warnings
        .is_empty());
}

/// F1：TrustPolicy 的注入点覆盖真实事实（Windows 集成测试依赖这一点）。
#[test]
fn trust_policy_overrides_patch_facts() {
    let mut p = TrustPolicy::production();
    p.overrides.dacl = Some(mihomo_default_dacl());
    let mut f = FakePipe::sidecar_ok();
    assert_eq!(
        p.decide_pipe(PipeKind::Sidecar, &mut f).unwrap_err().reason,
        "dacl-allows-foreign-instance"
    );

    let mut p = TrustPolicy::production();
    p.overrides.owner = Some(other());
    assert_eq!(
        p.decide_pipe(PipeKind::Sidecar, &mut FakePipe::sidecar_ok())
            .unwrap_err()
            .reason,
        "owner-mismatch"
    );

    let mut p = TrustPolicy::production();
    p.overrides.label = Some(LabelFact::Rid(0x1000));
    assert_eq!(
        p.decide_pipe(PipeKind::Sidecar, &mut FakePipe::sidecar_ok())
            .unwrap_err()
            .reason,
        "label-below-medium"
    );

    let mut p = TrustPolicy::production();
    p.overrides.security_err = Some(FactErr::new("GetSecurityInfo", 5));
    assert_eq!(
        p.decide_pipe(PipeKind::Sidecar, &mut FakePipe::sidecar_ok())
            .unwrap_err()
            .reason,
        "security-unreadable"
    );

    let mut p = TrustPolicy::production();
    p.overrides.l3 = Some(Ok(other()));
    p.overrides.l2_1 = Some(Ok("evil.exe".into()));
    p.overrides.server_pid = Some(Ok(1));
    let t = p
        .decide_pipe(PipeKind::Sidecar, &mut FakePipe::sidecar_ok())
        .unwrap();
    assert_eq!((t.server_pid, t.token), (Some(1), Some(other())));
    assert_eq!(t.warnings.len(), 2, "{:?}", t.warnings);

    // TCP：监听表 / 连接表 / 回落状态注入
    let mut p = TrustPolicy::production();
    p.overrides.listeners_v4 = Some(Ok(vec![row("127.0.0.1", 7)]));
    p.overrides.extra_listeners_v4 = vec![row("0.0.0.0", 8)];
    assert_eq!(
        p.decide_tcp(v4addr(), Ok(vec![]), Ok(vec![]), &mut FakeProc::mine())
            .clone_reject_code(),
        Some("listener-conflict")
    );
    p.overrides.extra_listeners_v4.clear();
    p.overrides.listeners_v6 = Some(Ok(vec![]));
    assert!(matches!(
        p.decide_tcp(v4addr(), Ok(vec![]), Ok(vec![]), &mut FakeProc::mine()),
        ListenerCheck::Verified { pid: 7, .. }
    ));
    p.overrides.conn_pid = Some(Ok(Some(9)));
    assert!(p.post_connect(7, Ok(Some(7))).is_err());
    p.overrides.conn_pid = None;
    assert!(p.post_connect(7, Ok(Some(7))).is_ok());
    p.overrides.wts = Some(WtsUser::NullSid);
    assert!(matches!(
        p.decide_tcp(v4addr(), Ok(vec![]), Ok(vec![]), &mut FakeProc::mine()),
        ListenerCheck::Verified { .. }
    ));
    p.overrides.fallback = Some(TcpFallback::Blocked("x".into()));
    assert!(p.overrides.fallback.is_some());
}

#[test]
fn generic_mapping() {
    assert_eq!(
        map_generic(GENERIC_WRITE) & FILE_CREATE_PIPE_INSTANCE,
        FILE_CREATE_PIPE_INSTANCE
    );
    assert_eq!(
        map_generic(GENERIC_ALL) & (WRITE_DAC | WRITE_OWNER),
        WRITE_DAC | WRITE_OWNER
    );
    assert_eq!(
        map_generic(0x8000_0000 | 0x2000_0000) & INSTANCE_OR_SD_WRITE,
        0
    );
}

#[test]
fn m3_fallback_rules() {
    let u = Untrusted {
        reason: "owner-mismatch",
        detail: String::new(),
        owner_sid: None,
        server_pid: None,
    };
    assert_eq!(
        tcp_fallback_after_pipes(&[]),
        TcpFallback::Allowed { denied_seen: false }
    );
    assert_eq!(
        tcp_fallback_after_pipes(&[PipeErr::NotFound, PipeErr::Busy]),
        TcpFallback::Allowed { denied_seen: false }
    );
    assert_eq!(
        tcp_fallback_after_pipes(&[PipeErr::NotFound, PipeErr::Denied]),
        TcpFallback::Allowed { denied_seen: true }
    );
    assert!(matches!(
        tcp_fallback_after_pipes(&[PipeErr::NotFound, PipeErr::Untrusted(u), PipeErr::NotFound]),
        TcpFallback::Blocked(_)
    ));
    assert!(matches!(
        tcp_fallback_after_pipes(&[PipeErr::Io("x".into())]),
        TcpFallback::Blocked(_)
    ));
    let ins = Untrusted {
        reason: "dacl-null",
        detail: String::new(),
        owner_sid: None,
        server_pid: None,
    };
    assert!(matches!(
        tcp_fallback_after_pipes(&[PipeErr::InsecureDacl(ins.clone()), PipeErr::NotFound]),
        TcpFallback::VerifiedOnly(_)
    ));
    // Untrusted 优先于 InsecureDacl
    let u2 = Untrusted {
        reason: "owner-mismatch",
        detail: String::new(),
        owner_sid: None,
        server_pid: None,
    };
    assert!(matches!(
        tcp_fallback_after_pipes(&[PipeErr::InsecureDacl(ins), PipeErr::Untrusted(u2)]),
        TcpFallback::Blocked(_)
    ));
}

/// Ellie §2-10：InsecureDacl 之后 TCP 只认 listener-verified。
#[test]
fn verified_only_fallback_plan() {
    let vo = TcpFallback::VerifiedOnly("insecure".into());
    let verified = ListenerCheck::Verified {
        pid: 7,
        image: "verge-mihomo.exe".into(),
    };
    assert_eq!(
        tcp_send_plan(&verified, false, false, &vo),
        TcpPlan::SendWithoutAuth
    );
    let unverified = ListenerCheck::Unverified {
        pid: Some(7),
        reasons: vec![],
        service_mode: true,
    };
    assert_eq!(
        tcp_send_plan(&unverified, false, true, &vo),
        TcpPlan::Refuse("insecure".into())
    );
}

// ---- TCP 规则 4 ----------------------------------------------------------------------------

#[derive(Clone)]
struct FakeProc {
    l2_1: Result<String, FactErr>,
    l2_2: Result<String, FactErr>,
    l2_3: Result<String, FactErr>,
    l3: Result<Ident, FactErr>,
    wts: WtsUser,
}

impl FakeProc {
    fn mine() -> Self {
        FakeProc {
            l2_1: Ok(r"C:\Program Files\Clash Verge\verge-mihomo.exe".into()),
            l2_2: Ok(r"\Device\HarddiskVolume3\Program Files\Clash Verge\verge-mihomo.exe".into()),
            l2_3: Ok("verge-mihomo.exe".into()),
            l3: Ok(me()),
            wts: WtsUser::User(me()),
        }
    }
}

impl ProcessFacts for FakeProc {
    fn image_win32_path(&mut self, _: u32) -> Result<String, FactErr> {
        self.l2_1.clone()
    }
    fn image_nt_path(&mut self, _: u32) -> Result<String, FactErr> {
        self.l2_2.clone()
    }
    fn image_snapshot_name(&mut self, _: u32) -> Result<String, FactErr> {
        self.l2_3.clone()
    }
    fn token_user(&mut self, _: u32) -> Result<Ident, FactErr> {
        self.l3.clone()
    }
    fn wts_user(&mut self, _: u32) -> WtsUser {
        self.wts.clone()
    }
}

fn row(ip: &str, pid: u32) -> ListenerRow {
    ListenerRow {
        ip: ip.parse().unwrap(),
        pid,
    }
}
fn v4addr() -> SocketAddr {
    "127.0.0.1:9097".parse().unwrap()
}

fn check(
    addr: SocketAddr,
    v4: Vec<ListenerRow>,
    v6: Vec<ListenerRow>,
    p: &FakeProc,
) -> ListenerCheck {
    decide_tcp_listener(addr, &Ok(v4), &Ok(v6), &images(), &mut p.clone())
}

#[test]
fn tcp_verified_requires_unique_whitelisted_me() {
    let c = check(
        v4addr(),
        vec![row("127.0.0.1", 7)],
        vec![],
        &FakeProc::mine(),
    );
    assert!(matches!(c, ListenerCheck::Verified { pid: 7, .. }), "{c:?}");
    assert_eq!(c.source_label(), Some("verge-tcp(listener-verified)"));
    // 0.0.0.0 也算
    let c = check(v4addr(), vec![row("0.0.0.0", 7)], vec![], &FakeProc::mine());
    assert!(matches!(c, ListenerCheck::Verified { .. }));
    // 同一 PID 同时监听 127.0.0.1 与 0.0.0.0 不算冲突
    let c = check(
        v4addr(),
        vec![row("0.0.0.0", 7), row("127.0.0.1", 7)],
        vec![],
        &FakeProc::mine(),
    );
    assert!(matches!(c, ListenerCheck::Verified { .. }));
    // IPv6
    let a6: SocketAddr = "[::1]:9097".parse().unwrap();
    let c = check(a6, vec![], vec![row("::", 7)], &FakeProc::mine());
    assert!(matches!(c, ListenerCheck::Verified { .. }));
}

#[test]
fn tcp_listener_conflict_and_unrelated_rows() {
    let c = check(
        v4addr(),
        vec![row("0.0.0.0", 7), row("127.0.0.1", 8)],
        vec![],
        &FakeProc::mine(),
    );
    assert_eq!(
        c,
        ListenerCheck::Rejected(TcpReject::ListenerConflict(vec![7, 8]))
    );
    // Ellie PR50 Q1（取代 S2 时的做法）：0.0.0.0 + [::] 组合若 PID 不同，也算冲突；PID 相同仍可验证。
    let c = check(
        v4addr(),
        vec![row("0.0.0.0", 7)],
        vec![row("::", 8)],
        &FakeProc::mine(),
    );
    assert_eq!(
        c,
        ListenerCheck::Rejected(TcpReject::ListenerConflict(vec![7, 8]))
    );
    let c = check(
        v4addr(),
        vec![row("0.0.0.0", 7)],
        vec![row("::", 7)],
        &FakeProc::mine(),
    );
    assert!(matches!(c, ListenerCheck::Verified { pid: 7, .. }), "{c:?}");
    // 绑定在其他具体地址上的监听者无关
    let c = check(
        v4addr(),
        vec![row("127.0.0.1", 7), row("192.168.1.5", 8)],
        vec![],
        &FakeProc::mine(),
    );
    assert!(matches!(c, ListenerCheck::Verified { pid: 7, .. }));
}

#[test]
fn tcp_not_listening_and_dual_stack_hint() {
    assert_eq!(
        check(v4addr(), vec![], vec![], &FakeProc::mine()),
        ListenerCheck::NotListening {
            ipv6_dual_stack_hint: false
        }
    );
    assert_eq!(
        check(v4addr(), vec![], vec![row("::", 7)], &FakeProc::mine()),
        ListenerCheck::NotListening {
            ipv6_dual_stack_hint: true
        }
    );
    assert_eq!(
        check(
            v4addr(),
            vec![row("192.168.1.5", 7)],
            vec![row("::1", 7)],
            &FakeProc::mine()
        ),
        ListenerCheck::NotListening {
            ipv6_dual_stack_hint: false
        }
    );
}

#[test]
fn tcp_rejects() {
    // F2：WTS 只作诊断；与 L3 矛盾时判为 unverified，不再单独拒绝。
    let p = FakeProc {
        wts: WtsUser::User(other()),
        ..FakeProc::mine()
    };
    assert!(matches!(
        check(v4addr(), vec![row("127.0.0.1", 7)], vec![], &p),
        ListenerCheck::Unverified { .. }
    ));
    let p = FakeProc {
        l3: Ok(other()),
        ..FakeProc::mine()
    };
    assert_eq!(
        check(v4addr(), vec![row("127.0.0.1", 7)], vec![], &p).clone_reject_code(),
        Some("token-mismatch")
    );
    let p = FakeProc {
        l2_1: Ok(r"C:\x\nc.exe".into()),
        ..FakeProc::mine()
    };
    assert_eq!(
        check(v4addr(), vec![row("127.0.0.1", 7)], vec![], &p).clone_reject_code(),
        Some("image-mismatch")
    );
    // ① 读不到时，②③ 只作诊断：不能据此拒绝，也不能据此验证
    let p = FakeProc {
        l2_1: Err(denied("OpenProcess")),
        l2_2: Ok(r"\Device\x\nc.exe".into()),
        ..FakeProc::mine()
    };
    assert!(matches!(
        check(v4addr(), vec![row("127.0.0.1", 7)], vec![], &p),
        ListenerCheck::Unverified { .. }
    ));
}

#[test]
fn tcp_unverified_branches() {
    let cases: Vec<(&str, FakeProc)> = vec![
        (
            "L3 ACCESS_DENIED",
            FakeProc {
                l3: Err(denied("OpenProcess")),
                ..FakeProc::mine()
            },
        ),
        (
            "PID 已退出",
            FakeProc {
                l2_1: Err(FactErr::new("OpenProcess", 87)),
                l3: Err(FactErr::new("OpenProcess", 87)),
                ..FakeProc::mine()
            },
        ),
        (
            "L2 ① 失败",
            FakeProc {
                l2_1: Err(denied("QueryFullProcessImageNameW")),
                ..FakeProc::mine()
            },
        ),
        (
            "SYSTEM 服务模式",
            FakeProc {
                l2_1: Err(denied("OpenProcess")),
                l3: Ok(system()),
                wts: WtsUser::NullSid,
                ..FakeProc::mine()
            },
        ),
        (
            "SYSTEM 服务模式（L3 可读、映像在白名单）",
            FakeProc {
                l3: Ok(system()),
                ..FakeProc::mine()
            },
        ),
        (
            "WTS 为 SYSTEM",
            FakeProc {
                wts: WtsUser::User(system()),
                ..FakeProc::mine()
            },
        ),
    ];
    for (name, p) in cases {
        let c = check(v4addr(), vec![row("127.0.0.1", 7)], vec![], &p);
        assert!(
            matches!(c, ListenerCheck::Unverified { pid: Some(7), .. }),
            "{name}: {c:?}"
        );
        assert_eq!(c.source_label(), Some("verge-tcp(listener-unverified)"));
    }
    let c = decide_tcp_listener(
        v4addr(),
        &Err(FactErr::new("GetExtendedTcpTable", 87)),
        &Ok(vec![]),
        &images(),
        &mut FakeProc::mine(),
    );
    assert!(matches!(c, ListenerCheck::Unverified { pid: None, .. }));
    // SYSTEM 服务模式标记
    let p = FakeProc {
        l3: Ok(system()),
        ..FakeProc::mine()
    };
    assert!(matches!(
        check(v4addr(), vec![row("127.0.0.1", 7)], vec![], &p),
        ListenerCheck::Unverified {
            service_mode: true,
            ..
        }
    ));
}

#[test]
fn tcp_f4_wts_unknown_does_not_reject_or_block_verification() {
    for w in [
        WtsUser::NullSid,
        WtsUser::SidCopyFailed,
        WtsUser::NotInSnapshot,
        WtsUser::Failed(FactErr::new("WTSEnumerateProcessesExW", 5)),
    ] {
        let p = FakeProc {
            wts: w.clone(),
            ..FakeProc::mine()
        };
        assert!(
            matches!(
                check(v4addr(), vec![row("127.0.0.1", 7)], vec![], &p),
                ListenerCheck::Verified { .. }
            ),
            "{w:?}"
        );
        let p = FakeProc {
            wts: w.clone(),
            l3: Err(denied("OpenProcess")),
            ..FakeProc::mine()
        };
        assert!(
            matches!(
                check(v4addr(), vec![row("127.0.0.1", 7)], vec![], &p),
                ListenerCheck::Unverified { .. }
            ),
            "{w:?}"
        );
    }
}

#[test]
fn post_connect_binding() {
    assert!(post_connect_check(7, Ok(Some(7))).is_ok());
    assert!(post_connect_check(7, Ok(Some(8))).is_err());
    assert!(post_connect_check(7, Ok(None)).is_err());
    assert!(post_connect_check(7, Err(FactErr::new("GetExtendedTcpTable", 5))).is_err());
}

#[test]
fn send_plan_matrix() {
    let verified = ListenerCheck::Verified {
        pid: 7,
        image: "verge-mihomo.exe".into(),
    };
    let unverified = ListenerCheck::Unverified {
        pid: Some(7),
        reasons: vec![],
        service_mode: false,
    };
    let rejected = ListenerCheck::Rejected(TcpReject::ListenerConflict(vec![1, 2]));
    let none = ListenerCheck::NotListening {
        ipv6_dual_stack_hint: true,
    };
    let ok = TcpFallback::Allowed { denied_seen: false };
    let denied = TcpFallback::Allowed { denied_seen: true };
    let blocked = TcpFallback::Blocked("x".into());

    // 闸门关闭（生产）：任何情况下都不会 SendBearer
    for c in [&verified, &unverified, &rejected, &none] {
        for s in [true, false] {
            for fb in [&ok, &denied, &blocked] {
                assert_ne!(tcp_send_plan(c, s, false, fb), TcpPlan::SendBearer);
            }
        }
    }
    assert_eq!(
        tcp_send_plan(&verified, false, false, &ok),
        TcpPlan::SendWithoutAuth
    );
    assert!(matches!(
        tcp_send_plan(&verified, true, false, &ok),
        TcpPlan::Refuse(_)
    ));
    // 闸门打开（仅测试）：只有 verified 才带 Bearer
    assert_eq!(
        tcp_send_plan(&verified, true, true, &ok),
        TcpPlan::SendBearer
    );
    assert_eq!(
        tcp_send_plan(&verified, true, true, &denied),
        TcpPlan::SendBearer
    );
    for c in [&unverified, &rejected, &none] {
        for s in [true, false] {
            assert!(
                matches!(tcp_send_plan(c, s, true, &ok), TcpPlan::Refuse(_)),
                "{c:?}"
            );
        }
    }
    // M-3：管道不可信时连 verified 也不回落
    for s in [true, false] {
        for g in [true, false] {
            assert!(matches!(
                tcp_send_plan(&verified, s, g, &blocked),
                TcpPlan::Refuse(_)
            ));
        }
    }
    if let TcpPlan::Refuse(m) = tcp_send_plan(&none, false, false, &ok) {
        assert!(m.contains("[::1]"), "{m}");
    }
}

impl ListenerCheck {
    fn clone_reject_code(&self) -> Option<&'static str> {
        match self {
            ListenerCheck::Rejected(r) => Some(r.code()),
            _ => None,
        }
    }
}

#[test]
fn file_name_handles_all_separators() {
    let m: HashMap<&str, &str> = [
        (r"C:\a\b.exe", "b.exe"),
        (r"\Device\HarddiskVolume3\a\b.exe", "b.exe"),
        ("a/b.exe", "b.exe"),
        ("b.exe", "b.exe"),
    ]
    .into_iter()
    .collect();
    for (k, v) in m {
        assert_eq!(file_name(k), v);
    }
}

/// Ellie PR50 Q1：目标为 IPv4 时，v6 表 `::` 上 PID 不同的监听者也算冲突。
#[test]
fn ipv6_unspecified_listener_with_other_pid_is_conflict_for_ipv4_target() {
    let p = FakeProc::mine();
    assert_eq!(
        check(v4addr(), vec![row("127.0.0.1", 7)], vec![row("::", 8)], &p).clone_reject_code(),
        Some("listener-conflict")
    );
    // 同一 PID 的双栈监听不算冲突
    assert!(matches!(
        check(v4addr(), vec![row("127.0.0.1", 7)], vec![row("::", 7)], &p),
        ListenerCheck::Verified { pid: 7, .. }
    ));
    // `::1`（非通配）不影响 127.0.0.1
    assert!(matches!(
        check(v4addr(), vec![row("127.0.0.1", 7)], vec![row("::1", 8)], &p),
        ListenerCheck::Verified { pid: 7, .. }
    ));
    // 只有 v6 通配、v4 没有监听：仍是 NotListening（带双栈提示），不凭 v6 行判 verified
    assert!(matches!(
        check(v4addr(), vec![], vec![row("::", 8)], &p),
        ListenerCheck::NotListening {
            ipv6_dual_stack_hint: true
        }
    ));
}

/// Ellie PR50 Q4：标签不带 NO_WRITE_UP（如 `S:(ML;;NR;;;ME)`）⇒ 拒绝，即使 RID 不低于 Medium。
#[test]
fn label_without_no_write_up_rejects() {
    // SYSTEM_MANDATORY_LABEL_NO_WRITE_UP（security.rs 读取标签时用到）
    assert_eq!(MANDATORY_NO_WRITE_UP, 0x1);
    for rid in [0x1000, MANDATORY_MEDIUM_RID, 0x3000] {
        let mut f = FakePipe::sidecar_ok().with_label(LabelFact::WithoutNoWriteUp(rid));
        let e = decide(PipeKind::Sidecar, &mut f).unwrap_err();
        assert_eq!(e.reason, "label-no-write-up", "0x{rid:X}");
        assert!(matches!(PipeErr::from_reject(e), PipeErr::InsecureDacl(_)));
    }
}

/// Ellie PR50 M-a：单次请求只能收紧 M-3 状态。
#[test]
fn single_request_can_only_tighten_fallback() {
    let allowed = || TcpFallback::Allowed { denied_seen: false };
    let vo = || TcpFallback::VerifiedOnly("v".into());
    let blocked = || TcpFallback::Blocked("b".into());
    // Blocked 之后：成功 / NotFound（= Allowed）、VerifiedOnly、Blocked 都不覆盖
    for new in [allowed(), vo(), blocked()] {
        assert_eq!(tighten_fallback(Some(&blocked()), new), None);
    }
    // VerifiedOnly 之后：只有 Blocked 能写入
    assert_eq!(tighten_fallback(Some(&vo()), allowed()), None);
    assert_eq!(tighten_fallback(Some(&vo()), vo()), None);
    assert_eq!(tighten_fallback(Some(&vo()), blocked()), Some(blocked()));
    // Allowed 之后：收紧可写
    assert_eq!(tighten_fallback(Some(&allowed()), allowed()), None);
    assert_eq!(tighten_fallback(Some(&allowed()), vo()), Some(vo()));
    assert_eq!(
        tighten_fallback(Some(&allowed()), blocked()),
        Some(blocked())
    );
    // 缓存为空：不写 Allowed（否则会跳过下一次完整探测），收紧可写
    assert_eq!(tighten_fallback(None, allowed()), None);
    assert_eq!(
        tighten_fallback(None, TcpFallback::Allowed { denied_seen: true }),
        None
    );
    assert_eq!(tighten_fallback(None, blocked()), Some(blocked()));
    // NotFound / Busy / 成功折算成的 Allowed 都不会放宽
    let nf = tcp_fallback_after_pipes(&[PipeErr::NotFound]);
    assert_eq!(tighten_fallback(Some(&blocked()), nf), None);
}
