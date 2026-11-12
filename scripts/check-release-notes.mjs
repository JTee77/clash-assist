#!/usr/bin/env node
/**
 * 发布说明文案检查（windows-plan r4 §1 决策 2、§5 R11）：
 * 发布说明不写任何关于系统安全拦截提示的文案（SmartScreen、Defender、智能应用控制、
 * “仍要运行”、未知发布者、未签名等）。release-windows.yml 在生成 release 说明前调用，
 * ci.yml 的 checks job 也会检查模板本身。
 *
 * 另外检查「平台改动标记」（文档 46，见 docs/windows-updater.md）：
 *   <!-- clash-assist:platforms macos=X.Y.Z windows=X.Y.Z -->
 * - 各平台的值来自仓库里的 release/platforms.json（版本 PR 里改），每次运行都会校验它：
 *   值为 X.Y.Z、不高于应用版本、至少一个等于应用版本、不低于 0.3.1（首个带这项逻辑的版本）；
 *   应用版本不高于 0.3.1 时两个值都必须等于应用版本；
 * - 文件名为 release-notes*.md 时必须恰好有一条标记；其他文件（如 portable.txt）有就检查；
 * - 模板（含 {{ 占位符）只接受 macos={{MACOS_VERSION}} windows={{WINDOWS_VERSION}}；
 * - 生成后的说明：macos / windows 各一次，值为 X.Y.Z；给了 --version 时还要求与 release/platforms.json 一致。
 *
 * 用法：node scripts/check-release-notes.mjs [--version X.Y.Z] <文件 ...>
 *       node scripts/check-release-notes.mjs --self-test
 * 退出码：0 = 通过；1 = 命中禁用词或无法读取；2 = 用法错误。
 * 只用 Node 内置模块。
 */

import { readFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
export const PLATFORMS_FILE = "release/platforms.json";

// 比较前统一转小写，并去掉空白（防止 “Smart Screen”“仍要 运行” 之类拆开写法）
export const BANNED = [
  "smartscreen",
  "defender",
  "smartappcontrol",
  "智能应用控制",
  "windows已保护你的电脑",
  "已保护你的电脑",
  "保护了你的电脑",
  "已保護你的電腦",
  "protectedyourpc",
  "仍要运行",
  "仍要執行",
  "runanyway",
  "未知发布者",
  "未知發行者",
  "unknownpublisher",
  "未签名",
  "未簽署",
  "unsigned",
  "代码签名",
  "codesigning",
  "安全警告",
  "securitywarning",
];

const squash = (s) => s.toLowerCase().replace(/\s+/gu, "");

/** 返回命中的禁用词列表（按 BANNED 顺序，去重）。 */
export function findBanned(text) {
  const t = squash(text);
  return BANNED.filter((w) => t.includes(squash(w)));
}

export const MARKER_KEY = "clash-assist:platforms";
export const MARKER_TEMPLATE = `<!-- ${MARKER_KEY} macos={{MACOS_VERSION}} windows={{WINDOWS_VERSION}} -->`;
/** 首个能读懂标记的版本：它及更早的版本两个平台都必须正常发布。 */
export const FIRST_MARKER_VERSION = "0.3.1";
const PLATFORMS = ["macos", "windows"];
const SEMVER = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/u;

function cmpVersion(a, b) {
  const x = a.split(".").map(Number);
  const y = b.split(".").map(Number);
  for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return x[i] < y[i] ? -1 : 1;
  return 0;
}

/**
 * 检查平台改动标记，返回错误列表（空 = 通过）。
 * required：没有标记是否算错；version：本版版本号（可选）。
 */
/** 校验 release/platforms.json 的内容（已解析的对象），返回错误列表。 */
export function checkPlatforms(obj, appVersion) {
  if (obj === null || typeof obj !== "object" || Array.isArray(obj)) return [`${PLATFORMS_FILE} 必须是 JSON 对象`];
  const errs = [];
  const keys = Object.keys(obj).sort();
  if (keys.join(",") !== [...PLATFORMS].sort().join(",")) {
    errs.push(`${PLATFORMS_FILE} 只能有 ${PLATFORMS.join(" / ")} 两项（实际：${keys.join(", ") || "空"}）`);
  }
  for (const k of PLATFORMS) {
    if (typeof obj[k] !== "string" || !SEMVER.test(obj[k])) errs.push(`${PLATFORMS_FILE} 的 ${k} 不是 X.Y.Z：${obj[k]}`);
  }
  if (errs.length) return errs;
  for (const k of PLATFORMS) {
    if (cmpVersion(obj[k], appVersion) > 0) errs.push(`${PLATFORMS_FILE} 的 ${k}=${obj[k]} 高于应用版本 ${appVersion}`);
  }
  if (cmpVersion(appVersion, FIRST_MARKER_VERSION) <= 0) {
    for (const k of PLATFORMS) {
      if (obj[k] !== appVersion) errs.push(`应用版本 ${appVersion} 不高于 ${FIRST_MARKER_VERSION}（旧客户端读不懂标记），两个平台都必须正常发布：${k} 应为 ${appVersion}`);
    }
    return errs;
  }
  if (!PLATFORMS.some((k) => obj[k] === appVersion)) {
    errs.push(`${PLATFORMS_FILE} 至少一个平台应等于应用版本 ${appVersion}（版本 PR 忘了更新？）`);
  }
  for (const k of PLATFORMS) {
    if (cmpVersion(obj[k], FIRST_MARKER_VERSION) < 0) errs.push(`${PLATFORMS_FILE} 的 ${k}=${obj[k]} 低于 ${FIRST_MARKER_VERSION}`);
  }
  return errs;
}

/** 从标记行解析出 { macos, windows }（只在 checkMarker 通过后调用）。 */
export function markerValues(text) {
  const line = text.split(/\r?\n/u).find((l) => l.includes(MARKER_KEY));
  return Object.fromEntries(/clash-assist:platforms\s+(.*?)\s*-->/u.exec(line)[1].split(/\s+/u).map((t) => t.split("=")));
}

export function checkMarker(text, { required = false, version = null } = {}) {
  const count = text.split(MARKER_KEY).length - 1;
  if (count === 0) return required ? [`缺少平台改动标记（${MARKER_TEMPLATE}）`] : [];
  if (count > 1) return [`平台改动标记出现了 ${count} 次，只能有一次`];
  const line = text.split(/\r?\n/u).find((l) => l.includes(MARKER_KEY)).trim();
  if (text.includes("{{")) {
    return line === MARKER_TEMPLATE ? [] : [`模板里的平台改动标记只能是默认值：${MARKER_TEMPLATE}`];
  }
  const m = /^<!--\s*clash-assist:platforms\s+(.*?)\s*-->$/u.exec(line);
  if (!m) return [`平台改动标记格式不对：${line}`];
  const seen = {};
  const errs = [];
  for (const tok of m[1].split(/\s+/u)) {
    const [k, v, extra] = tok.split("=");
    if (!PLATFORMS.includes(k) || v === undefined || extra !== undefined) {
      errs.push(`平台改动标记里有无法识别的项：${tok}`);
    } else if (k in seen) {
      errs.push(`平台改动标记里 ${k} 重复`);
    } else if (!SEMVER.test(v)) {
      errs.push(`平台改动标记里 ${k} 的值不是 X.Y.Z：${v}`);
    } else {
      seen[k] = v;
    }
  }
  for (const k of PLATFORMS) if (!(k in seen) && !errs.length) errs.push(`平台改动标记缺少 ${k}`);
  if (errs.length || !version) return errs;
  for (const k of PLATFORMS) {
    if (cmpVersion(seen[k], version) > 0) errs.push(`平台改动标记里 ${k}=${seen[k]} 高于本版 ${version}`);
  }
  if (!PLATFORMS.some((k) => seen[k] === version)) {
    errs.push(`平台改动标记里至少一个平台应等于本版 ${version}`);
  }
  return errs;
}

function selfTest() {
  const bad = [
    "If Windows SmartScreen blocks it, click More info.",
    "Windows 已保护你的电脑，请点击“仍要运行”。",
    "Smart Screen may warn you",
    "安装包未签名",
    "Microsoft Defender 可能会拦截",
  ];
  const good = ["安装版，按当前用户安装，不需要管理员权限。", "gh attestation verify x -R a/b"];
  let ok = true;
  for (const s of bad) {
    if (findBanned(s).length === 0) {
      console.error(`self-test FAIL：应命中却未命中：${s}`);
      ok = false;
    }
  }
  for (const s of good) {
    const hit = findBanned(s);
    if (hit.length) {
      console.error(`self-test FAIL：不应命中却命中 ${hit.join(", ")}：${s}`);
      ok = false;
    }
  }
  const mk = (body) => `## Clash伴侣 0.3.2\n\n${body}\n\n### Windows\n`;
  const markerCases = [
    // [说明, 文本, 选项, 是否应通过]
    ["模板默认值", `## Clash伴侣 {{VERSION}}\n\n${MARKER_TEMPLATE}\n`, { required: true }, true],
    ["模板里写死跳过 Mac", "## {{VERSION}}\n<!-- clash-assist:platforms macos=0.3.1 windows={{WINDOWS_VERSION}} -->", { required: true }, false],
    ["模板仍用旧占位符", "## {{VERSION}}\n<!-- clash-assist:platforms macos={{VERSION}} windows={{VERSION}} -->", { required: true }, false],
    ["生成后两平台都改动", mk("<!-- clash-assist:platforms macos=0.3.2 windows=0.3.2 -->"), { required: true, version: "0.3.2" }, true],
    ["Mac 无变化", mk("<!-- clash-assist:platforms macos=0.3.1 windows=0.3.2 -->"), { required: true, version: "0.3.2" }, true],
    ["CRLF", mk("<!-- clash-assist:platforms macos=0.3.1 windows=0.3.2 -->").replace(/\n/gu, "\r\n"), { required: true, version: "0.3.2" }, true],
    ["缺标记", mk(""), { required: true }, false],
    ["非 release-notes 文件可以没有", "便携版说明", {}, true],
    ["出现两次", mk(`${MARKER_TEMPLATE.replace(/\{\{\w+\}\}/gu, "0.3.2")}\n<!-- clash-assist:platforms macos=0.3.2 windows=0.3.2 -->`), { required: true }, false],
    ["缺 windows", mk("<!-- clash-assist:platforms macos=0.3.2 -->"), { required: true }, false],
    ["未知平台", mk("<!-- clash-assist:platforms macos=0.3.2 windows=0.3.2 linux=0.3.2 -->"), { required: true }, false],
    ["v 前缀", mk("<!-- clash-assist:platforms macos=v0.3.2 windows=0.3.2 -->"), { required: true }, false],
    ["重复键", mk("<!-- clash-assist:platforms macos=0.3.2 macos=0.3.1 windows=0.3.2 -->"), { required: true }, false],
    ["未闭合", mk("<!-- clash-assist:platforms macos=0.3.2 windows=0.3.2"), { required: true }, false],
    ["高于本版", mk("<!-- clash-assist:platforms macos=0.3.3 windows=0.3.2 -->"), { required: true, version: "0.3.2" }, false],
    ["两个都不等于本版", mk("<!-- clash-assist:platforms macos=0.3.1 windows=0.3.1 -->"), { required: true, version: "0.3.2" }, false],
    ["0.10.0 > 0.9.0", mk("<!-- clash-assist:platforms macos=0.9.0 windows=0.10.0 -->"), { required: true, version: "0.10.0" }, true],
  ];
  for (const [name, text, opts, pass] of markerCases) {
    const errs = checkMarker(text, opts);
    if ((errs.length === 0) !== pass) {
      console.error(`self-test FAIL：标记「${name}」应${pass ? "通过" : "失败"}：${errs.join("；") || "（无错误）"}`);
      ok = false;
    }
  }
  const platCases = [
    // [说明, 对象, 应用版本, 是否应通过]
    ["0.3.0 两平台都等于", { macos: "0.3.0", windows: "0.3.0" }, "0.3.0", true],
    ["0.3.1 两平台都等于", { macos: "0.3.1", windows: "0.3.1" }, "0.3.1", true],
    ["0.3.1 不允许跳过", { macos: "0.3.0", windows: "0.3.1" }, "0.3.1", false],
    ["0.3.2 Mac 无变化", { macos: "0.3.1", windows: "0.3.2" }, "0.3.2", true],
    ["0.3.2 忘了更新", { macos: "0.3.1", windows: "0.3.1" }, "0.3.2", false],
    ["高于应用版本", { macos: "0.3.3", windows: "0.3.2" }, "0.3.2", false],
    ["低于 0.3.1", { macos: "0.3.0", windows: "0.3.2" }, "0.3.2", false],
    ["v 前缀", { macos: "v0.3.2", windows: "0.3.2" }, "0.3.2", false],
    ["缺 windows", { macos: "0.3.2" }, "0.3.2", false],
    ["多余键", { macos: "0.3.2", windows: "0.3.2", linux: "0.3.2" }, "0.3.2", false],
    ["不是对象", ["0.3.2"], "0.3.2", false],
    ["0.10.0 Windows 无变化", { macos: "0.10.0", windows: "0.9.0" }, "0.10.0", true],
  ];
  for (const [name, obj, app, pass] of platCases) {
    const errs = checkPlatforms(obj, app);
    if ((errs.length === 0) !== pass) {
      console.error(`self-test FAIL：platforms「${name}」应${pass ? "通过" : "失败"}：${errs.join("；") || "（无错误）"}`);
      ok = false;
    }
  }
  // 仓库里的模板本身：默认值、无禁用词
  const tpl = readFileSync(new URL("./release-notes-windows.md", import.meta.url), "utf8");
  if (checkMarker(tpl, { required: true }).length || findBanned(tpl).length) {
    console.error("self-test FAIL：scripts/release-notes-windows.md 未通过检查");
    ok = false;
  }
  console.log(ok ? "check-release-notes self-test：全部通过" : "check-release-notes self-test：失败");
  return ok ? 0 : 1;
}

function main(argv) {
  if (argv.length === 1 && argv[0] === "--self-test") return selfTest();
  let version = null;
  if (argv[0] === "--version") {
    version = argv[1] ?? "";
    argv = argv.slice(2);
  }
  if (argv.length === 0 || argv.some((a) => a.startsWith("-")) || (version !== null && !SEMVER.test(version))) {
    console.error("用法：node scripts/check-release-notes.mjs [--version X.Y.Z] <文件 ...> | --self-test");
    return 2;
  }
  let rc = 0;
  // release/platforms.json：每次都校验（应用版本取 --version，否则取 package.json）
  let platforms = null;
  try {
    const appVersion = version ?? JSON.parse(readFileSync(join(ROOT, "package.json"), "utf8")).version;
    platforms = JSON.parse(readFileSync(join(ROOT, PLATFORMS_FILE), "utf8"));
    const errs = checkPlatforms(platforms, appVersion);
    for (const e of errs) console.error(`check-release-notes：${e}`);
    if (errs.length) rc = 1;
    else console.log(`check-release-notes：${PLATFORMS_FILE} 通过（${appVersion}：macos=${platforms.macos} windows=${platforms.windows}）`);
  } catch (e) {
    console.error(`check-release-notes：无法读取 ${PLATFORMS_FILE} 或 package.json（${e.code ?? e.message}）`);
    rc = 1;
  }
  for (const f of argv) {
    let text;
    try {
      text = readFileSync(f, "utf8");
    } catch (e) {
      console.error(`check-release-notes：无法读取 ${f}（${e.code ?? e.message}）`);
      rc = 1;
      continue;
    }
    const hit = findBanned(text);
    const markerErrs = checkMarker(text, { required: /^release-notes.*\.md$/u.test(basename(f)), version });
    if (hit.length) {
      console.error(`check-release-notes：${f} 含禁用文案：${hit.join(", ")}`);
      rc = 1;
    }
    // 生成后的说明（给了 --version）：标记必须与 release/platforms.json 一致
    if (version && platforms && !markerErrs.length && text.includes(MARKER_KEY) && !text.includes("{{")) {
      const got = markerValues(text);
      for (const k of PLATFORMS) {
        if (got[k] !== platforms[k]) markerErrs.push(`标记 ${k}=${got[k]} 与 ${PLATFORMS_FILE} 的 ${platforms[k]} 不一致`);
      }
    }
    for (const e of markerErrs) console.error(`check-release-notes：${f}：${e}`);
    if (markerErrs.length) rc = 1;
    if (!hit.length && !markerErrs.length) {
      console.log(`check-release-notes：${f} 通过`);
    }
  }
  return rc;
}

process.exitCode = main(process.argv.slice(2));
