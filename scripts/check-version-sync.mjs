#!/usr/bin/env node
/**
 * 版本一致性检查（windows-plan r4 §2.8 / §3，S-9）。
 *
 * 要求以下来源的版本号完全一致：
 *   - package.json                  的 "version"
 *   - src-tauri/Cargo.toml          的 [package] version
 *   - src-tauri/tauri.conf.json     的 "version"
 *   - src-tauri/tauri.<平台>.conf.json（若存在且写了 "version"，平台覆盖不得改版本号）
 *   - git tag（仅在提供时）：`--tag vX.Y.Z`，或 GitHub Actions 中 GITHUB_REF_TYPE=tag 时取
 *     GITHUB_REF_NAME。tag 必须形如 vX.Y.Z（与 release-windows.yml 的 tag 过滤一致）。
 *
 * 用法：node scripts/check-version-sync.mjs [--tag vX.Y.Z] [--root <仓库根目录>]
 * 退出码：0 = 一致；1 = 不一致或无法读取；2 = 用法错误。
 * 只用 Node 内置模块，不引入依赖。
 */

import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

function usage(msg) {
  if (msg) console.error(`check-version-sync: ${msg}`);
  console.error("用法：node scripts/check-version-sync.mjs [--tag vX.Y.Z] [--root <dir>]");
  process.exit(2);
}

let root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
let tag;
const argv = process.argv.slice(2);
for (let i = 0; i < argv.length; i++) {
  const a = argv[i];
  if (a === "--tag" || a === "--root") {
    const v = argv[++i];
    if (v === undefined || v === "") usage(`${a} 需要一个值`);
    if (a === "--tag") tag = v;
    else root = resolve(v);
  } else if (a === "-h" || a === "--help") {
    usage();
  } else {
    usage(`未知参数：${a}`);
  }
}
if (tag === undefined && process.env.GITHUB_REF_TYPE === "tag" && process.env.GITHUB_REF_NAME) {
  tag = process.env.GITHUB_REF_NAME;
}

const problems = [];
/** @type {{ source: string, version: string }[]} */
const found = [];

function read(rel) {
  const p = join(root, rel);
  try {
    return readFileSync(p, "utf8");
  } catch (e) {
    problems.push(`${rel}: 无法读取（${e.code ?? e.message}）`);
    return undefined;
  }
}

function fromJson(rel, { optional = false } = {}) {
  const text = read(rel);
  if (text === undefined) return;
  let obj;
  try {
    obj = JSON.parse(text);
  } catch (e) {
    problems.push(`${rel}: JSON 解析失败（${e.message}）`);
    return;
  }
  if (typeof obj.version === "string" && obj.version !== "") {
    found.push({ source: rel, version: obj.version });
  } else if (!optional) {
    problems.push(`${rel}: 缺少字符串类型的 "version" 字段`);
  }
}

/** 只取 [package] 表中的 version；不支持 version.workspace = true（本仓库不是 workspace）。 */
function fromCargoToml(rel) {
  const text = read(rel);
  if (text === undefined) return;
  let inPackage = false;
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.replace(/\s+#.*$/, "").trim();
    const header = line.match(/^\[([^\]]+)\]$/);
    if (header) {
      inPackage = header[1].trim() === "package";
      continue;
    }
    if (!inPackage) continue;
    const m = line.match(/^version\s*=\s*"([^"]*)"$/);
    if (m) {
      found.push({ source: `${rel} [package]`, version: m[1] });
      return;
    }
  }
  problems.push(`${rel}: [package] 中没有 version = "..."`);
}

fromJson("package.json");
fromCargoToml("src-tauri/Cargo.toml");
fromJson("src-tauri/tauri.conf.json");

const tauriDir = join(root, "src-tauri");
if (existsSync(tauriDir)) {
  for (const name of readdirSync(tauriDir).sort()) {
    if (/^tauri\.[A-Za-z0-9_-]+\.conf\.json$/.test(name)) {
      fromJson(`src-tauri/${name}`, { optional: true });
    }
  }
}

if (tag !== undefined) {
  const m = tag.match(/^v(\d+\.\d+\.\d+)$/);
  if (m) found.push({ source: `git tag ${tag}`, version: m[1] });
  else problems.push(`git tag "${tag}" 不是 vX.Y.Z 格式`);
}

const versions = new Set(found.map((f) => f.version));
if (versions.size > 1) {
  problems.push(
    `版本号不一致：\n${found.map((f) => `    ${f.source.padEnd(32)} ${f.version}`).join("\n")}`,
  );
}

if (problems.length > 0) {
  console.error("check-version-sync: 失败");
  for (const p of problems) console.error(`  - ${p}`);
  process.exit(1);
}

const [version] = versions;
console.log(`check-version-sync: 通过，版本 ${version}（${found.map((f) => f.source).join("、")}）`);
