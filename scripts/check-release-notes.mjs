#!/usr/bin/env node
/**
 * 发布说明文案检查（windows-plan r4 §1 决策 2、§5 R11）：
 * 发布说明不写任何关于系统安全拦截提示的文案（SmartScreen、Defender、智能应用控制、
 * “仍要运行”、未知发布者、未签名等）。release-windows.yml 在生成 release 说明前调用，
 * ci.yml 的 checks job 也会检查模板本身。
 *
 * 用法：node scripts/check-release-notes.mjs <文件 ...>
 *       node scripts/check-release-notes.mjs --self-test
 * 退出码：0 = 通过；1 = 命中禁用词或无法读取；2 = 用法错误。
 * 只用 Node 内置模块。
 */

import { readFileSync } from "node:fs";

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
  console.log(ok ? "check-release-notes self-test：全部通过" : "check-release-notes self-test：失败");
  return ok ? 0 : 1;
}

function main(argv) {
  if (argv.length === 1 && argv[0] === "--self-test") return selfTest();
  if (argv.length === 0 || argv.some((a) => a.startsWith("-"))) {
    console.error("用法：node scripts/check-release-notes.mjs <文件 ...> | --self-test");
    return 2;
  }
  let rc = 0;
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
    if (hit.length) {
      console.error(`check-release-notes：${f} 含禁用文案：${hit.join(", ")}`);
      rc = 1;
    } else {
      console.log(`check-release-notes：${f} 通过`);
    }
  }
  return rc;
}

process.exitCode = main(process.argv.slice(2));
