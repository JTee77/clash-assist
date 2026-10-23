// leakMatrix.ts — 纯函数判定矩阵：把 IPv6 / WebRTC 两类"总黄色"检测改造成
// 给出确定 pass/fail 的可测逻辑。不碰网络、不读全局，只吃信号、吐结论，
// 因此可被 vitest 完整覆盖（见 leakMatrix.test.ts）。
//
// 核心思想（确定性来源）：
//  · IPv6：单看 (直连v6, 代理v6) 分不清"真旁路"与"TUN 全局接管"。引入 IPv4 交叉
//    验证——若走代理后 IPv4 出口发生变化，说明直连/代理是两条真不同的路径，此时
//    v6 才谈得上"旁路"；若 IPv4 两路完全一致，则流量被统一隧道接管，v6 同址是健康。
//  · WebRTC：真正的泄漏信号是"候选里出现 ≠ 已知代理出口的公网地址"。把代理出口 IP
//    喂进来比对，就能把"仅内网候选(安全)/公网候选=代理出口(安全)/公网候选≠出口(泄漏)"
//    三类明确分开，而不是统统 warn。

import type { CheckLevel } from "./types";

export type LeakVerdict = {
  level: CheckLevel; // pass | warn | fail | unknown
  conclusion: string;
  suggestion?: string;
  /** 判定依据，逐条可折叠展示（诚实文化） */
  rationale: string[];
};

export type Ipv6Probe = {
  /** 是否已连上代理口（mixed-port 可用）；false 时无法做直连/代理对照 */
  proxyConfigured: boolean;
  directV4: string | null;
  proxiedV4: string | null;
  directV6: string | null;
  proxiedV6: string | null;
};

/** 代理是否被证明能改变 IPv4 出口（直连/代理确为两条路径）。 */
function proxyChangesIpv4(p: Ipv6Probe): boolean {
  return (
    p.proxyConfigured &&
    !!p.directV4 &&
    !!p.proxiedV4 &&
    p.directV4 !== p.proxiedV4
  );
}

/**
 * IPv6 泄漏判定矩阵。输入是四个出口观测 + 是否已连代理，输出确定结论。
 * 优先级自上而下，命中即返回。
 */
export function classifyIpv6Leak(p: Ipv6Probe): LeakVerdict {
  const rationale: string[] = [];
  const sameV4 =
    p.proxyConfigured && !!p.directV4 && !!p.proxiedV4 && p.directV4 === p.proxiedV4;

  // 1) 两侧都没有 IPv6 —— 无从泄漏，确定健康。
  if (!p.directV6 && !p.proxiedV6) {
    return {
      level: "pass",
      conclusion: "直连与代理均探测不到可用 IPv6 出口，不存在 IPv6 旁路。",
      rationale: ["无 IPv6 出口可评估"],
    };
  }

  // 2) 没连上代理口 —— 无法对照，诚实标未知（不误判）。
  if (!p.proxyConfigured) {
    return {
      level: "unknown",
      conclusion: p.directV6
        ? `直连可达 IPv6（${p.directV6}），但尚未连上代理口，无法对照。`
        : "IPv6 状态不明（未连上代理口）。",
      suggestion: "请先点「获取节点」确保已连上软件，再重测。",
      rationale: ["缺少代理侧对照"],
    };
  }

  rationale.push(
    `IPv4：直连 ${p.directV4 ?? "无"} / 代理 ${p.proxiedV4 ?? "无"}`,
    `IPv6：直连 ${p.directV6 ?? "无"} / 代理 ${p.proxiedV6 ?? "无"}`,
  );

  // 3) 直连有 v6、代理侧无 —— 经典旁路签名：v6 绕过了代理。确定判危险。
  if (p.directV6 && !p.proxiedV6) {
    rationale.push(
      sameV4
        ? "注：IPv4 两路同源，但代理侧反而拿不到 v6，直连 v6 走了独立路径"
        : proxyChangesIpv4(p)
          ? "代理已改变 IPv4 出口，直连 v6 却无对应代理 v6 → 明确旁路"
          : "无法确认代理是否接管 v6",
    );
    return {
      level: "fail",
      conclusion: `直连能拿到 IPv6（${p.directV6}），代理侧没有 —— IPv6 很可能绕过代理直连出去。`,
      suggestion:
        "若期望全局走代理：检查客户端的 IPv6 / TUN 设置并强制接管，或暂时关闭系统 IPv6。",
      rationale,
    };
  }

  // 4) 仅代理侧有 v6，直连无 —— 代理提供了 v6 且直连不泄漏。确定健康。
  if (!p.directV6 && p.proxiedV6) {
    return {
      level: "pass",
      conclusion: `仅代理侧有 IPv6（${p.proxiedV6}），直连无 —— 未见直连旁路。`,
      rationale: [...rationale, "直连无 v6 出口，代理侧 v6 由隧道提供"],
    };
  }

  // 5) 两侧都有 v6
  // 5a) 同一个 v6 地址
  if (p.directV6 && p.proxiedV6 && p.directV6 === p.proxiedV6) {
    if (proxyChangesIpv4(p)) {
      // v4 走代理会变，v6 却直连/代理同址 → 说明 v6 也被统一出口接管
      return {
        level: "pass",
        conclusion: `IPv4 经代理后出口改变，IPv6 直连与代理同址（${p.directV6}）—— IPv6 已被统一接管，未见旁路。`,
        rationale: [...rationale, "v4 分离证明代理生效；v6 同址=同走隧道"],
      };
    }
    // v4 也两路同源 → 全局/TUN 接管，一切走同一条路，健康
    return {
      level: "pass",
      conclusion: "IPv4 与 IPv6 在直连/代理两侧出口完全一致 —— 流量被统一隧道接管，无旁路。",
      rationale: [...rationale, "v4/v6 均同出口，判定为全局接管（健康）"],
    };
  }

  // 5b) 不同 v6 地址 —— 直连用了自己的 v6 路由，与代理不同 → 旁路/泄漏
  return {
    level: "fail",
    conclusion: `直连 IPv6（${p.directV6}）与代理 IPv6（${p.proxiedV6}）不一致 —— 存在独立直连 IPv6 出口，可能暴露真实身份。`,
    suggestion: "在意暴露时关闭系统 IPv6，或强制 TUN 接管 IPv6；改完重测。",
    rationale: [...rationale, "v6 两路不同地址 = 直连另有一条 v6 出口"],
  };
}

export type WebRtcCandidateScope = "private" | "public" | "unknown";
export type WebRtcCandidateType =
  | "host"
  | "srflx"
  | "prflx"
  | "relay"
  | "unknown";

export type WebRtcCandidate = {
  type: WebRtcCandidateType;
  address: string;
  scope: WebRtcCandidateScope;
};

export type WebRtcSignal = {
  /** WebView 是否有 RTCPeerConnection */
  apiAvailable: boolean;
  /** 收集过程是否抛错 */
  gatherFailed: boolean;
  candidates: WebRtcCandidate[];
  /** 已知代理侧公网出口 IP（来自 exit-ip 卡）；用于比对判定候选是否为出口 */
  proxyExitIps?: string[];
  /** 候选地址 → 归属国（无法取得的地址省略/置 null）。供归属地判定用。 */
  geoCountryByAddr?: Record<string, string | null>;
  /** 真实归属国（由系统区域推断），用于区分"供应商基础设施"与"真实 ISP 暴露" */
  realCountry?: string | null;
};

/** 从系统区域（navigator.language，如 zh-CN）推断"真实归属国"；取不到返回 null。 */
export function localeCountry(): string | null {
  try {
    const lang = typeof navigator !== "undefined" ? navigator.language || "" : "";
    const m = /^[a-zA-Z]{2,3}-([A-Za-z]{2})\b/.exec(lang);
    return m ? m[1].toUpperCase() : null;
  } catch {
    return null;
  }
}

/**
 * ICE 候选地址 → scope 归类（纯函数，可单测）。
 * 修复1：mDNS 混淆主机名（如 `a1b2….local`）不是可路由公网 IP，旧内联判定因它含 "."
 * 而误判为 public，导致 WebRTC 卡把隐私地址也列进「暴露的公网地址」。
 * 修复2：TUN/fake-ip 场景——mihomo TUN 默认用 198.18.0.0/15（RFC 2544 基准段，永不公网路由），
 * ICE host 候选会枚举到 TUN 网卡地址，误判 public 造成「公网暴露」假警报；
 * 同理补 CGNAT 100.64.0.0/10（运营商级 NAT，含 Tailscale 类网段）与
 * IPv4-mapped IPv6（::ffff:x.x.x.x，浏览器对 v4 映射地址的标准写法）。
 */
export function isPrivateOrLocalAddress(ip: string): boolean {
  const t = ip.trim().toLowerCase();
  if (t === "::1" || t === "0.0.0.0") return true;
  if (t.startsWith("fe80:")) return true; // link-local
  if (t.startsWith("fc") || t.startsWith("fd")) return true; // ULA rough
  // IPv4-mapped IPv6（::ffff:a.b.c.d）按映射的 v4 判定
  if (t.startsWith("::ffff:") && t.includes(".")) {
    return isPrivateOrLocalAddress(t.slice("::ffff:".length));
  }
  const m = t.match(/^(\d+)\.(\d+)\.(\d+)\.(\d+)$/);
  if (!m) return false;
  const a = Number(m[1]);
  const b = Number(m[2]);
  if (a === 10) return true;
  if (a === 127) return true;
  if (a === 100 && b >= 64 && b <= 127) return true; // CGNAT 100.64.0.0/10
  if (a === 169 && b === 254) return true;
  if (a === 172 && b >= 16 && b <= 31) return true;
  if (a === 192 && b === 168) return true;
  if (a === 198 && (b === 18 || b === 19)) return true; // fake-ip / 基准段 198.18.0.0/15
  return false;
}

/** mDNS/混淆名（.local 结尾）或根本不是 IP 字面量的主机名 → 视为本地，非公网。 */
export function isMdnsOrLocalName(address: string): boolean {
  const t = address.trim().toLowerCase();
  if (t.endsWith(".local")) return true;
  // 既不含 "." 也不含 ":" 的裸主机名不是 IP，按本地处理。
  return !t.includes(".") && !t.includes(":");
}

/** 判定 ICE 候选地址的可见性范围。 */
export function candidateScope(address: string): WebRtcCandidateScope {
  if (isPrivateOrLocalAddress(address)) return "private";
  if (isMdnsOrLocalName(address)) return "private";
  if (address.includes(".") || address.includes(":")) return "public";
  return "unknown";
}

/** 是否为"可安全忽略"的地址：等于已知代理出口。 */
function matchesProxyExit(addr: string, exits: string[]): boolean {
  return exits.includes(addr);
}

/**
 * WebRTC 泄漏判定。确定性来源：把候选公网地址与"已知代理出口"比对。
 *  · 等于出口 → 正常（STUN 也走了代理）
 *  · 是公网但不等于出口 → 归属地判定：与真实归属不同（供应商基础设施）→ pass；
 *    与真实归属一致或归属未知 → 疑似暴露真实地址 → fail
 *  · 拿到公网但根本没有出口可比 → warn（无法确证）
 *  · 仅内网/本地候选 → pass
 *
 * 归属地判定存在的意义：v6 普及后，"候选 ≠ 出口"不再等于泄漏——节点/供应商
 * 常有 v4 出口 + v6 出口（甚至解析基础设施在别国）的组合，字符串比对必然误报。
 */
export function classifyWebRtc(s: WebRtcSignal): LeakVerdict {
  if (!s.apiAvailable) {
    return {
      level: "unknown",
      conclusion: "当前 WebView 无 RTCPeerConnection，无法收集 ICE 候选（不等于无泄漏）。",
      suggestion: "可在系统浏览器用 chrome://webrtc-internals 或 BrowserLeaks 复核。",
      rationale: ["API 缺失"],
    };
  }
  if (s.gatherFailed) {
    return {
      level: "warn",
      conclusion: "本次未能收集 WebRTC 候选（可能被策略拦截）。失败 ≠ 无泄漏。",
      rationale: ["收集抛错"],
    };
  }

  const exits = (s.proxyExitIps ?? []).filter(Boolean);
  if (s.candidates.length === 0) {
    return {
      level: "unknown",
      conclusion: "未收集到 ICE 候选（可能被策略拦截或网络限制）。",
      rationale: ["候选为空"],
    };
  }

  // 公网、非 relay、且不等于已知代理出口的候选 → 疑似真实地址暴露
  const exposed = s.candidates.filter(
    (c) =>
      c.scope === "public" &&
      c.type !== "relay" &&
      (exits.length === 0 || !matchesProxyExit(c.address, exits)),
  );
  const onlyWhenNoExit = exits.length === 0;

  if (exposed.length > 0) {
    const addrs = [...new Set(exposed.map((c) => c.address))];
    const geo = s.geoCountryByAddr ?? {};
    const real = s.realCountry ?? null;
    // 归属地判定：归属已知且 ≠ 真实归属 → 供应商基础设施，不算暴露
    const benign: string[] = [];
    const suspect: string[] = [];
    for (const a of addrs) {
      const cc = geo[a] ?? null;
      if (cc && real && cc !== real) benign.push(`${a}（${cc}）`);
      else suspect.push(cc && real ? `${a}（${cc}）` : a);
    }
    if (benign.length > 0 && suspect.length === 0) {
      return {
        level: "pass",
        conclusion: `WebRTC 公网候选（${benign.join(", ")}）≠ 代理出口，但归属地与真实归属（${real}）不同 —— 属节点/供应商基础设施，未泄漏真实身份。`,
        rationale: [
          `公网候选 ${addrs}`,
          `归属 ${[...new Set(Object.values(geo).filter(Boolean))]}`,
          `真实归属 ${real}`,
        ],
      };
    }
    if (onlyWhenNoExit) {
      // 没有代理出口可比对 → 无法确证，诚实标警
      return {
        level: "warn",
        conclusion: `收集到公网候选（${addrs.join(", ")}），但当前无代理出口 IP 可比对，无法确证是否泄漏。`,
        suggestion: "先完成环境检查拿到出口 IP，或到系统浏览器用 BrowserLeaks 复核。",
        rationale: [`公网候选 ${addrs}`, "缺少可比对的出口 IP"],
      };
    }
    return {
      level: "fail",
      conclusion: `WebRTC 暴露了 ≠ 代理出口的公网地址（${suspect.join(", ")}）—— 可能泄漏真实 IP。${
        benign.length ? `另有 ${benign.length} 个候选属供应商基础设施，不算暴露。` : ""
      }`,
      suggestion: "在意暴露时禁用 WebRTC，或仅允许代理路径；客户端可开 WebRTC 泄露防护后重测。",
      rationale: [`疑似真实地址 ${suspect}`, `代理出口 ${exits.join(", ")}`],
    };
  }

  const privOnly = s.candidates.every((c) => c.scope !== "public");
  return {
    level: "pass",
    conclusion: privOnly
      ? "WebRTC 仅内网/本地候选，未见公网地址暴露。"
      : "WebRTC 公网候选均等于代理出口 —— 未泄漏真实身份。",
    rationale: [`候选 ${s.candidates.length}`, `出口 ${exits.join(", ") || "无"}`],
  };
}
