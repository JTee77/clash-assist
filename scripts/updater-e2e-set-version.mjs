#!/usr/bin/env node
/**
 * updater-e2e.yml 专用：在 CI 的一次性 checkout 中临时改写版本号（windows-plan r4 §6 M5：
 * 「分别以 0.3.0 和 0.3.1 带 updater-e2e feature 构建两个安装包，版本号只在这个 job 中临时改写」）。
 *
 * 改写四处，且每处必须恰好命中一次（否则失败，不做模糊替换）：
 *   - package.json                 顶层 "version"
 *   - src-tauri/tauri.conf.json    顶层 "version"
 *   - src-tauri/Cargo.toml         [package] 表中的 version
 *   - src-tauri/Cargo.lock         name = "clash-assist" 的 [[package]] 的 version（让 --locked 仍然成立）
 * 改写后由调用方再跑 scripts/check-version-sync.mjs 复核。
 *
 * 用法：node scripts/updater-e2e-set-version.mjs <X.Y.Z> [--root <dir>]
 *       node scripts/updater-e2e-set-version.mjs --self-test
 * 退出码：0 = 成功；1 = 失败；2 = 用法错误。只用 Node 内置模块。
 */
import { cpSync, mkdtempSync, readFileSync, rmSync, writeFileSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const here = dirname(fileURLToPath(import.meta.url));
const VERSION_RE = /^\d+\.\d+\.\d+$/;

function replaceOnce(text, re, replacer, what) {
  const hits = [...text.matchAll(new RegExp(re.source, re.flags.includes("g") ? re.flags : re.flags + "g"))];
  if (hits.length !== 1) throw new Error(`${what}：应恰好命中 1 处，实际 ${hits.length} 处`);
  return text.replace(re, replacer);
}

function setJsonVersion(text, ver, what) {
  const obj = JSON.parse(text);
  if (typeof obj.version !== "string") throw new Error(`${what}：缺少字符串 "version"`);
  // 只改顶层 version 那一行，保留原有格式
  const out = replaceOnce(text, /^(\s{2}"version":\s*")[^"]*(")/m, `$1${ver}$2`, what);
  if (JSON.parse(out).version !== ver) throw new Error(`${what}：改写后 version 不是 ${ver}`);
  return out;
}

function setCargoTomlVersion(text, ver) {
  const out = replaceOnce(text, /(^\[package\]\r?\n(?:(?!^\[).*\r?\n)*?^version\s*=\s*")[^"]*(")/m, `$1${ver}$2`, "Cargo.toml [package] version");
  return out;
}

function setCargoLockVersion(text, ver) {
  return replaceOnce(text, /(^\[\[package\]\]\r?\nname = "clash-assist"\r?\nversion = ")[^"]*(")/m, `$1${ver}$2`, "Cargo.lock clash-assist version");
}

export function setVersion(root, ver) {
  if (!VERSION_RE.test(ver)) throw new Error(`版本号必须形如 X.Y.Z：${ver}`);
  const edits = [
    ["package.json", (t) => setJsonVersion(t, ver, "package.json")],
    ["src-tauri/tauri.conf.json", (t) => setJsonVersion(t, ver, "tauri.conf.json")],
    ["src-tauri/Cargo.toml", (t) => setCargoTomlVersion(t, ver)],
    ["src-tauri/Cargo.lock", (t) => setCargoLockVersion(t, ver)],
  ];
  // 先全部算好再写，任何一处失败都不留下半改的树
  const out = edits.map(([rel, f]) => [rel, f(readFileSync(join(root, rel), "utf8"))]);
  for (const [rel, text] of out) writeFileSync(join(root, rel), text);
  for (const [rel] of out) console.log(`${rel} → ${ver}`);
}

function selfTest() {
  const repo = resolve(here, "..");
  let fail = 0;
  const check = (cond, msg) => {
    console.log(`${cond ? "ok  " : "FAIL"} ${msg}`);
    if (!cond) fail = 1;
  };
  const tmp = mkdtempSync(join(tmpdir(), "ca-setver-"));
  try {
    const files = ["package.json", "src-tauri/tauri.conf.json", "src-tauri/Cargo.toml", "src-tauri/Cargo.lock"];
    // tauri.<平台>.conf.json 也复制，check-version-sync 会读它们
    for (const rel of [...files, "src-tauri/tauri.windows.conf.json"]) {
      mkdirSync(dirname(join(tmp, rel)), { recursive: true });
      cpSync(join(repo, rel), join(tmp, rel));
    }
    const before = Object.fromEntries(files.map((f) => [f, readFileSync(join(tmp, f), "utf8")]));
    setVersion(tmp, "0.3.1");
    const sync = spawnSync(process.execPath, [join(here, "check-version-sync.mjs"), "--root", tmp, "--tag", "v0.3.1"], { encoding: "utf8" });
    check(sync.status === 0, `改写后 check-version-sync --tag v0.3.1 通过（${(sync.stdout + sync.stderr).trim()}）`);
    for (const f of files) {
      const a = before[f].split("\n");
      const b = readFileSync(join(tmp, f), "utf8").split("\n");
      const changed = a.filter((l, i) => l !== b[i]).length;
      check(a.length === b.length && changed === 1, `${f}：只改了 1 行（实际 ${changed} 行）`);
    }
    const lock = readFileSync(join(tmp, "src-tauri/Cargo.lock"), "utf8");
    check(/name = "clash-assist"\r?\nversion = "0\.3\.1"/.test(lock), "Cargo.lock 中 clash-assist 为 0.3.1");
    let threw = false;
    try { setVersion(tmp, "0.3"); } catch { threw = true; }
    check(threw, "拒绝非法版本号 0.3");
    // 命中 0 处时失败且不写任何文件
    writeFileSync(join(tmp, "src-tauri/Cargo.lock"), lock.replace('name = "clash-assist"', 'name = "renamed"'));
    const pj = readFileSync(join(tmp, "package.json"), "utf8");
    threw = false;
    try { setVersion(tmp, "0.3.2"); } catch { threw = true; }
    check(threw && readFileSync(join(tmp, "package.json"), "utf8") === pj, "Cargo.lock 找不到包时失败，且不留下半改的文件");
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
  console.log(`updater-e2e-set-version 自测：${fail ? "失败" : "全部通过"}`);
  process.exit(fail);
}

const argv = process.argv.slice(2);
if (argv[0] === "--self-test" && argv.length === 1) selfTest();
else {
  let root = resolve(here, "..");
  const pos = [];
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === "--root") root = resolve(argv[++i] ?? "");
    else pos.push(argv[i]);
  }
  if (pos.length !== 1) {
    console.error("用法：node scripts/updater-e2e-set-version.mjs <X.Y.Z> [--root <dir>] | --self-test");
    process.exit(2);
  }
  try {
    setVersion(root, pos[0]);
  } catch (e) {
    console.error(`updater-e2e-set-version：${e.message}`);
    process.exit(1);
  }
}
