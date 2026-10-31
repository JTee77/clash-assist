#!/usr/bin/env python3
"""检查 src-tauri/windows/installer.nsi（自带的 Tauri NSIS 模板，M4b）没有偏离上游。

NSIS 3.10 的 `RmDir /r` 会跟随目录 junction，Tauri 模板「删除应用数据」分支里的两行
`RmDir /r "$APPDATA\\${BUNDLEID}"` / `RmDir /r "$LOCALAPPDATA\\${BUNDLEID}"` 因此换成了
hooks.nsh 的 `!insertmacro CA_DELETE_TAURI_APP_DATA`（不跟随链接）。本检查保证：

1. 把 `; >>> clash-assist M4b` … `; <<< clash-assist M4b` 这一段还原成上游原文后，
   与 tauri-cli v2.11.4 的 installer.nsi 逐字节一致（SHA-256，换行统一为 LF）：
   除了这一处，模板没有别的改动；
2. 这一段恰好出现一次，里面就是那条 !insertmacro，且仍在「勾选删除、非 /UPDATE」分支、
   `SetShellVarContext current` 之后；
3. package.json 锁定的 @tauri-apps/cli 仍是 v2.11.4（升级 CLI 时必须同步重做模板）；
4. tauri.windows.conf.json 用的就是这份模板和 hooks.nsh。

`--fetch-upstream`（M4d，release-windows.yml 演练时运行）：另外从固定的上游提交
UPSTREAM_COMMIT 下载上游模板，与「去掉 M4b 段、换回上游两行」后的 installer.nsi 逐字比对，
不一致时打印 unified diff 并失败；同时确认下载内容的 SHA-256 就是 UPSTREAM_SHA256。
重新同步模板的步骤见 docs/windows-nsis-template.md。

用法：python3 scripts/check-nsis-template.py [--self-test | --fetch-upstream]
"""
import difflib
import hashlib
import json
import re
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TEMPLATE = ROOT / "src-tauri/windows/installer.nsi"
CONF = ROOT / "src-tauri/tauri.windows.conf.json"
PACKAGE = ROOT / "package.json"

UPSTREAM_VERSION = "2.11.4"
# tauri-apps/tauri @ tauri-cli-v2.11.4: crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi
# tag tauri-cli-v2.11.4 指向的完整提交（gh api repos/tauri-apps/tauri/commits/tauri-cli-v2.11.4）
UPSTREAM_COMMIT = "8909f221d1515955fc843808032bdc5d62209c96"
UPSTREAM_PATH = "crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi"
UPSTREAM_URL = f"https://raw.githubusercontent.com/tauri-apps/tauri/{UPSTREAM_COMMIT}/{UPSTREAM_PATH}"
UPSTREAM_SHA256 = "20f4ecc730defb71f1342eaeaec4021df13be3d843abba0effe88ea5835fa079"
UPSTREAM_LINES = (
    '    RmDir /r "$APPDATA\\${BUNDLEID}"\n'
    '    RmDir /r "$LOCALAPPDATA\\${BUNDLEID}"\n'
)
BLOCK = re.compile(r"^    ; >>> clash-assist M4b\b.*?^    ; <<< clash-assist M4b\n", re.S | re.M)
CONTEXT = (
    "  ${If} $DeleteAppDataCheckboxState = 1\n"
    "  ${AndIf} $UpdateMode <> 1\n"
)


def check(template: str, conf: dict, package: dict) -> list:
    errs = []
    t = template.replace("\r\n", "\n")
    blocks = BLOCK.findall(t)
    if len(blocks) != 1:
        errs.append(f"installer.nsi：clash-assist M4b 段应恰好 1 处，实际 {len(blocks)} 处")
    else:
        body = [l.strip() for l in blocks[0].splitlines() if l.strip() and not l.strip().startswith(";")]
        if body != ["!insertmacro CA_DELETE_TAURI_APP_DATA"]:
            errs.append(f"installer.nsi：M4b 段里只能有 !insertmacro CA_DELETE_TAURI_APP_DATA，实际 {body}")
        start = t.index(blocks[0])
        before = t[:start]
        branch = before.rfind(CONTEXT)
        if branch < 0 or "${EndIf}" in before[branch:] or "SetShellVarContext current\n" not in before[branch:]:
            errs.append("installer.nsi：M4b 段必须在「勾选删除、非 /UPDATE」分支内、SetShellVarContext current 之后")
        restored = BLOCK.sub(lambda _m: UPSTREAM_LINES, t, count=1)
        got = hashlib.sha256(restored.encode("utf-8")).hexdigest()
        if got != UPSTREAM_SHA256:
            errs.append(
                f"installer.nsi：除 M4b 段外与 tauri-cli v{UPSTREAM_VERSION} 的模板不一致（还原后 SHA-256 {got}）"
            )
    cli = (package.get("devDependencies") or {}).get("@tauri-apps/cli") or (package.get("dependencies") or {}).get(
        "@tauri-apps/cli"
    )
    if cli != UPSTREAM_VERSION:
        errs.append(
            f"package.json：@tauri-apps/cli 为 {cli!r}，模板基于 v{UPSTREAM_VERSION}；升级 CLI 时要按新版模板重做 installer.nsi 并更新本检查"
        )
    nsis = (((conf.get("bundle") or {}).get("windows") or {}).get("nsis")) or {}
    if nsis.get("template") != "windows/installer.nsi":
        errs.append("tauri.windows.conf.json：bundle.windows.nsis.template 必须是 windows/installer.nsi")
    if nsis.get("installerHooks") != "windows/hooks.nsh":
        errs.append("tauri.windows.conf.json：bundle.windows.nsis.installerHooks 必须是 windows/hooks.nsh")
    return errs


def restore_upstream(template: str) -> str:
    """去掉 M4b 段、换回上游原文（换行统一为 LF）。"""
    return BLOCK.sub(lambda _m: UPSTREAM_LINES, template.replace("\r\n", "\n"), count=1)


def compare_upstream(template: str, upstream: str) -> list:
    errs = []
    up = upstream.replace("\r\n", "\n")
    got = hashlib.sha256(up.encode("utf-8")).hexdigest()
    if got != UPSTREAM_SHA256:
        errs.append(f"上游模板（{UPSTREAM_COMMIT}）的 SHA-256 为 {got}，与固定的 UPSTREAM_SHA256 不一致")
    ours = restore_upstream(template)
    if ours != up:
        diff = difflib.unified_diff(
            up.splitlines(keepends=True),
            ours.splitlines(keepends=True),
            f"upstream@{UPSTREAM_COMMIT[:7]}/installer.nsi",
            "src-tauri/windows/installer.nsi（去掉 M4b 段）",
        )
        print("".join(diff))
        errs.append(f"installer.nsi 去掉 M4b 段后与上游 {UPSTREAM_COMMIT} 的模板不一致（diff 见上）")
    return errs


def self_test(template: str, conf: dict, package: dict) -> int:
    import copy

    base = check(template, conf, package)
    if base:
        print("自测前提不成立：当前文件本身没通过", *base, sep="\n  ")
        return 1
    cases = [
        ("M4b 段外改了一行", template.replace("SetAutoClose true", "SetAutoClose false", 1), conf, package, "不一致"),
        ("M4b 段换回 RmDir /r", BLOCK.sub(lambda _m: "    ; >>> clash-assist M4b\n" + UPSTREAM_LINES + "    ; <<< clash-assist M4b\n", template), conf, package, "只能有"),
        ("M4b 段删掉", BLOCK.sub("", template), conf, package, "恰好 1 处"),
        ("M4b 段挪出删除分支", BLOCK.sub("", template).replace("SectionEnd\n\nFunction RestorePreviousInstallLocation", "SectionEnd\n" + BLOCK.search(template).group(0) + "\nFunction RestorePreviousInstallLocation", 1), conf, package, "分支内"),
        ("CRLF 换行照样通过", template.replace("\n", "\r\n"), conf, package, None),
    ]
    p2 = copy.deepcopy(package)
    p2.setdefault("devDependencies", {})["@tauri-apps/cli"] = "2.12.0"
    p2.get("dependencies", {}).pop("@tauri-apps/cli", None)
    cases.append(("CLI 升级未同步", template, conf, p2, "@tauri-apps/cli"))
    c2 = copy.deepcopy(conf)
    del c2["bundle"]["windows"]["nsis"]["template"]
    cases.append(("配置没用这份模板", template, c2, package, "template"))
    upstream = restore_upstream(template)
    up_cases = [
        ("上游比对：一致", template, upstream, None),
        ("上游比对：段外改动", template.replace("SetAutoClose true", "SetAutoClose false", 1), upstream, "不一致（diff"),
        ("上游比对：上游内容被换", template, upstream + "; extra\n", "SHA-256"),
    ]
    bad = 0
    for name, t, u, want in up_cases:
        errs = compare_upstream(t, u)
        ok = (not errs) if want is None else any(want in e for e in errs)
        print(("ok  " if ok else "FAIL"), name, "" if ok else errs)
        bad += not ok
    for name, t, c, p, want in cases:
        errs = check(t, c, p)
        ok = (not errs) if want is None else any(want in e for e in errs)
        print(("ok  " if ok else "FAIL"), name, "" if ok else errs)
        bad += not ok
    print("check-nsis-template 自测：" + ("全部通过" if not bad else f"{bad} 项失败"))
    return 1 if bad else 0


def main() -> int:
    template = TEMPLATE.read_text(encoding="utf-8")
    conf = json.loads(CONF.read_text(encoding="utf-8"))
    package = json.loads(PACKAGE.read_text(encoding="utf-8"))
    args = sys.argv[1:]
    if args == ["--self-test"]:
        return self_test(template, conf, package)
    if args not in ([], ["--fetch-upstream"]):
        print("用法：check-nsis-template.py [--self-test | --fetch-upstream]", file=sys.stderr)
        return 2
    errs = check(template, conf, package)
    if args == ["--fetch-upstream"]:
        print(f"下载上游模板：{UPSTREAM_URL}")
        with urllib.request.urlopen(UPSTREAM_URL, timeout=60) as r:
            upstream = r.read().decode("utf-8")
        errs += compare_upstream(template, upstream)
    if errs:
        for e in errs:
            print(f"::error::{e}")
        return 1
    print(f"check-nsis-template: 通过（installer.nsi 仅 M4b 段与 tauri-cli v{UPSTREAM_VERSION} 不同）")
    if args == ["--fetch-upstream"]:
        print(f"check-nsis-template: 与上游 {UPSTREAM_COMMIT} 逐字一致（M4b 段除外）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
