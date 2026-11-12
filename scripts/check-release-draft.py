#!/usr/bin/env python3
"""发布前闸门：核对某个 tag 的 draft release 是否可以发布（fail-closed）。

由 .github/workflows/publish-release.yml 调用。本脚本不联网、不需要令牌，只读
workflow 用 `gh api` 抓下来的快照文件，全部检查通过才返回 0：

  1. tag 严格形如 vX.Y.Z（semver，无前导零）；
  2. tag_name 等于该 tag 的 release 恰好一个，且仍是 draft、不是 prerelease；
  3. 资产恰好是这四个，名字与版本号一致，多一个、少一个、重名都失败：
       Clash.Assist_<ver>_x64-setup.exe     （release-windows.yml 写入）
       Clash.Assist_<ver>_x64-portable.zip  （release-windows.yml 写入）
       SHA256SUMS.txt                       （release-windows.yml 写入，已做 attestation）
       Clash.Assist_<ver>_aarch64.dmg       （人工上传；任何 .dmg 都计数，必须恰好一个）
     未知 / 多余资产一律失败：开了 immutable releases 后，发布出去的资产无法再删改；
     多余文件不在 SHA256SUMS 和 provenance 里，也可能被客户端或用户误选。确需新资产时改本脚本的清单。
  4. 每个资产 state == uploaded、size > 0、GitHub digest 形如 sha256:<64 位十六进制>；
  5. 下载下来的 SHA256SUMS.txt 本身的 sha256 等于其 GitHub digest，且内容恰好两行，
     分别对应 setup.exe 与便携 zip，哈希等于这两个资产的 GitHub digest
     （即 draft 里的 Windows 包就是 attest 过的那两个；不改动 SHA256SUMS.txt）；
  6. 若给了 --baseline（第一次快照的 assets.json）：release id 与每个资产的
     id / 名字 / 大小 / digest / state 必须与第一次快照完全一致（发布前复读，缩小竞态窗口）。

用法：
  python3 scripts/check-release-draft.py --tag vX.Y.Z --releases R.json --assets A.json --sums SHA256SUMS.txt \
      [--baseline-releases R0.json --baseline-assets A0.json] [--summary FILE]
  python3 scripts/check-release-draft.py --self-test
R.json / A.json 可以是 `gh api --paginate --slurp` 的分页数组（数组的数组），也可以是平铺数组。
退出码：0 = 可以发布；1 = 不可发布；2 = 用法错误 / 文件无法读取。
只用 Python 标准库。
"""

from __future__ import annotations

import argparse
import contextlib
import copy
import hashlib
import io
import json
import re
import sys
import tempfile
from pathlib import Path

TAG_RE = re.compile(r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")
DIGEST_RE = re.compile(r"sha256:[0-9a-f]{64}")
SUMS_NAME = "SHA256SUMS.txt"
FIXTURES = Path(__file__).resolve().parent / "tests" / "release-draft"


class UsageError(Exception):
    pass


def expected_names(version: str) -> dict[str, str]:
    return {
        "setup": f"Clash.Assist_{version}_x64-setup.exe",
        "zip": f"Clash.Assist_{version}_x64-portable.zip",
        "sums": SUMS_NAME,
        "dmg": f"Clash.Assist_{version}_aarch64.dmg",
    }


def flatten_pages(data, what: str) -> list:
    """`gh api --paginate --slurp` 给出数组的数组；平铺数组也接受。"""
    if not isinstance(data, list):
        raise UsageError(f"{what} 必须是 JSON 数组")
    if all(isinstance(x, list) for x in data):
        return [y for page in data for y in page]
    return data


def load_json(path: Path, what: str):
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as e:
        raise UsageError(f"无法读取 {what} {path}：{e}") from e


def find_release(releases: list, tag: str, errs: list[str]) -> dict | None:
    matches = [r for r in releases if isinstance(r, dict) and r.get("tag_name") == tag]
    if len(matches) != 1:
        errs.append(f"tag_name 为 {tag} 的 release 应恰好一个，实际 {len(matches)} 个"
                    + ("（还没有 draft：先推 tag，等 release-windows 写好 draft）" if not matches else ""))
        return None
    rel = matches[0]
    if rel.get("draft") is not True:
        errs.append(f"release {tag}（id={rel.get('id')}）不是 draft（draft={rel.get('draft')!r}），拒绝再次发布")
    if rel.get("prerelease") is not False:
        errs.append(f"release {tag} 的 prerelease={rel.get('prerelease')!r}，正式版必须为 false")
    if not isinstance(rel.get("id"), int):
        errs.append(f"release {tag} 的 id 无效：{rel.get('id')!r}")
    return rel


def check_assets(assets: list, version: str, errs: list[str]) -> dict[str, dict]:
    names = expected_names(version)
    by_name: dict[str, list[dict]] = {}
    for a in assets:
        if not isinstance(a, dict) or not isinstance(a.get("name"), str):
            errs.append(f"资产条目格式不对：{a!r}")
            continue
        by_name.setdefault(a["name"], []).append(a)

    dmgs = [a for a in assets if isinstance(a, dict) and str(a.get("name", "")).lower().endswith(".dmg")]
    if len(dmgs) != 1:
        errs.append(f"应恰好有一个 .dmg（{names['dmg']}），实际 {len(dmgs)} 个："
                    f"{sorted(str(a.get('name')) for a in dmgs)}")

    found: dict[str, dict] = {}
    for key, name in names.items():
        got = by_name.get(name, [])
        if len(got) != 1:
            errs.append(f"资产 {name} 应恰好一个，实际 {len(got)} 个")
        else:
            found[key] = got[0]

    extras = sorted(n for n in by_name if n not in names.values())
    if extras:
        errs.append(f"有清单之外的资产（fail-closed，发布后无法删除）：{extras}")

    for a in assets:
        if not isinstance(a, dict):
            continue
        n = a.get("name")
        if a.get("state") != "uploaded":
            errs.append(f"资产 {n} 的 state={a.get('state')!r}，必须是 uploaded")
        d = a.get("digest")
        if not isinstance(d, str) or not DIGEST_RE.fullmatch(d):
            errs.append(f"资产 {n} 没有有效的 GitHub digest（sha256:...），实际：{d!r}")
        size = a.get("size")
        if not isinstance(size, int) or isinstance(size, bool) or size <= 0:
            errs.append(f"资产 {n} 的 size 无效：{size!r}")
        if not isinstance(a.get("id"), int):
            errs.append(f"资产 {n} 的 id 无效：{a.get('id')!r}")
    return found


def check_sums(sums_bytes: bytes, found: dict[str, dict], version: str, errs: list[str]) -> None:
    names = expected_names(version)
    sums_asset = found.get("sums")
    if sums_asset is None:
        return  # 缺失已在 check_assets 记错
    want = sums_asset.get("digest")
    got = "sha256:" + hashlib.sha256(sums_bytes).hexdigest()
    if got != want:
        errs.append(f"下载的 {SUMS_NAME} 的 sha256 是 {got}，与 GitHub digest {want!r} 不一致")
        return
    try:
        text = sums_bytes.decode("utf-8")
    except UnicodeDecodeError:
        errs.append(f"{SUMS_NAME} 不是 UTF-8 文本")
        return
    lines = [l for l in text.split("\n") if l != ""]
    entries: dict[str, str] = {}
    for l in lines:
        m = re.fullmatch(r"([0-9a-f]{64})  (\S+)", l)
        if not m:
            errs.append(f"{SUMS_NAME} 有无法解析的行：{l!r}")
            return
        if m.group(2) in entries:
            errs.append(f"{SUMS_NAME} 里 {m.group(2)} 出现了两次")
            return
        entries[m.group(2)] = m.group(1)
    want_names = {names["setup"], names["zip"]}
    if set(entries) != want_names:
        errs.append(f"{SUMS_NAME} 应恰好列出 {sorted(want_names)}，实际：{sorted(entries)}")
        return
    for key in ("setup", "zip"):
        a = found.get(key)
        if a is None:
            continue
        if a.get("digest") != "sha256:" + entries[names[key]]:
            errs.append(f"{names[key]} 的 GitHub digest {a.get('digest')!r} 与 {SUMS_NAME} 里的 "
                        f"sha256:{entries[names[key]]} 不一致（draft 里的包不是 attest 过的那个）")


def asset_fingerprint(assets: list) -> list[tuple]:
    return sorted(
        (a.get("id"), a.get("name"), a.get("size"), a.get("digest"), a.get("state"))
        for a in assets if isinstance(a, dict)
    )


def evaluate(tag: str, releases_raw, assets_raw, sums_bytes: bytes,
             baseline_releases_raw=None, baseline_assets_raw=None) -> tuple[list[str], dict]:
    """返回 (错误列表, 信息)。错误列表为空才可以发布。"""
    errs: list[str] = []
    info: dict = {}
    if not isinstance(tag, str) or not TAG_RE.fullmatch(tag):
        return [f"tag 必须严格形如 vX.Y.Z（semver，无前导零），实际：{tag!r}"], info
    version = tag[1:]
    info["version"] = version
    releases = flatten_pages(releases_raw, "releases")
    assets = flatten_pages(assets_raw, "assets")
    rel = find_release(releases, tag, errs)
    if rel is not None:
        info["release_id"] = rel.get("id")
        inline = rel.get("assets")
        if isinstance(inline, list) and inline:
            if {a.get("id") for a in inline if isinstance(a, dict)} != {a.get("id") for a in assets if isinstance(a, dict)}:
                errs.append("release 对象里的资产与资产列表接口给出的不一致（快照期间有变化？）")
    found = check_assets(assets, version, errs)
    info["assets"] = {k: {"name": v.get("name"), "digest": v.get("digest"), "size": v.get("size")} for k, v in found.items()}
    check_sums(sums_bytes, found, version, errs)

    if (baseline_releases_raw is None) != (baseline_assets_raw is None):
        raise UsageError("--baseline-releases 与 --baseline-assets 必须同时给出")
    if baseline_assets_raw is not None:
        b_rel = [r for r in flatten_pages(baseline_releases_raw, "baseline releases")
                 if isinstance(r, dict) and r.get("tag_name") == tag]
        b_id = b_rel[0].get("id") if len(b_rel) == 1 else None
        if rel is None or b_id != rel.get("id"):
            errs.append(f"复读时 release id 与第一次快照不一致（之前 {b_id!r}，现在 {info.get('release_id')!r}）")
        if asset_fingerprint(flatten_pages(baseline_assets_raw, "baseline assets")) != asset_fingerprint(assets):
            errs.append("复读时资产（id / 名字 / 大小 / digest / state）与第一次检查时不一致，拒绝发布")
    return errs, info


def write_summary(path: Path, tag: str, errs: list[str], info: dict, rereading: bool) -> None:
    out = [f"### 发布前检查 {tag}" + ("（发布前复读）" if rereading else ""), ""]
    out.append("结果：**" + ("通过" if not errs else "不通过，未发布") + "**")
    out.append("")
    if info.get("assets"):
        out += ["| 资产 | 大小 | GitHub digest |", "| --- | ---: | --- |"]
        for k in ("setup", "zip", "sums", "dmg"):
            a = info["assets"].get(k)
            if a:
                out.append(f"| `{a['name']}` | {a['size']} | `{a['digest']}` |")
        out.append("")
    dmg = (info.get("assets") or {}).get("dmg")
    if dmg:
        out.append(f"DMG digest（请与本机构建的 DMG 人工核对）：`{dmg['digest']}`")
        out.append("")
    for e in errs:
        out.append(f"- {e}")
    with path.open("a", encoding="utf-8") as f:
        f.write("\n".join(out) + "\n")


def run(args) -> int:
    try:
        releases = load_json(args.releases, "releases")
        assets = load_json(args.assets, "assets")
        try:
            sums = args.sums.read_bytes()
        except OSError as e:
            raise UsageError(f"无法读取 {args.sums}：{e}") from e
        b_rel = load_json(args.baseline_releases, "baseline releases") if args.baseline_releases else None
        b_ast = load_json(args.baseline_assets, "baseline assets") if args.baseline_assets else None
        errs, info = evaluate(args.tag, releases, assets, sums, b_rel, b_ast)
    except UsageError as e:
        print(f"check-release-draft：{e}", file=sys.stderr)
        return 2
    rereading = args.baseline_assets is not None
    if args.summary:
        write_summary(args.summary, args.tag, errs, info, rereading)
    for k, a in sorted((info.get("assets") or {}).items()):
        print(f"check-release-draft：{a['name']}  size={a['size']}  {a['digest']}")
    if errs:
        for e in errs:
            print(f"::error::{e}")
        print(f"check-release-draft：{args.tag} 不可发布（{len(errs)} 项不通过）")
        return 1
    print(f"check-release-draft：{args.tag} 通过（release id={info['release_id']}"
          + ("，与第一次快照一致" if rereading else "") + "）")
    return 0


# ---------------------------------------------------------------------------
# 自测：基于 scripts/tests/release-draft/ 下的 fixture（形状取自 v0.3.0 的真实 API 输出）
# ---------------------------------------------------------------------------

def self_test() -> int:
    fail = 0
    base_releases = load_json(FIXTURES / "releases.json", "fixture releases")
    base_assets = load_json(FIXTURES / "assets.json", "fixture assets")
    base_sums = (FIXTURES / "SHA256SUMS.txt").read_bytes()
    tag = "v0.3.1"

    def page(x):
        return [x]  # 模拟 --slurp 的分页形状

    def rel_of(rs):
        return next(r for r in rs if r["tag_name"] == tag)

    def asset(as_, suffix):
        return next(a for a in as_ if a["name"].endswith(suffix))

    def case(label, mutate=None, *, want_ok=False, want=None, tag_=tag, baseline=None, flat=False):
        nonlocal fail
        rs, as_, sums = copy.deepcopy(base_releases), copy.deepcopy(base_assets), base_sums
        if mutate:
            r = mutate(rs, as_)
            if isinstance(r, bytes):
                sums = r
        b = None
        if baseline:
            b_rs, b_as = copy.deepcopy(base_releases), copy.deepcopy(base_assets)
            baseline(b_rs, b_as)
            b = (page(b_rs), page(b_as))
        wrap = (lambda x: x) if flat else page
        errs, _ = evaluate(tag_, wrap(rs), wrap(as_), sums, *(b or (None, None)))
        ok = (not errs) if want_ok else (bool(errs) and (want is None or any(want in e for e in errs)))
        if ok:
            print(f"ok   {label}" + ("" if want_ok else f"（{errs[0] if len(errs) == 1 else '; '.join(errs)}）"))
        else:
            print(f"FAIL {label}：{errs or '没有报错'}")
            fail = 1

    case("全部正确时通过", want_ok=True)
    case("与一致的快照复读比对时通过", want_ok=True, baseline=lambda r, a: None)
    case("平铺（非分页）数组也接受", want_ok=True, flat=True)
    case("缺 DMG", lambda r, a: a.remove(asset(a, ".dmg")), want="恰好有一个 .dmg")
    case("两个 DMG", lambda r, a: a.append(dict(asset(a, ".dmg"), id=9, name="Clash.Assist_0.3.1_aarch64 (1).dmg")),
         want="恰好有一个 .dmg")
    case("DMG 版本号不对", lambda r, a: asset(a, ".dmg").__setitem__("name", "Clash.Assist_0.3.0_aarch64.dmg"),
         want="Clash.Assist_0.3.1_aarch64.dmg 应恰好一个")
    case("tag 与资产版本不符（v0.3.2）", tag_="v0.3.2", want="tag_name 为 v0.3.2")
    case("tag 格式不对（v0.3）", tag_="v0.3", want="严格形如")
    case("tag 带前导零（v0.03.1）", tag_="v0.03.1", want="严格形如")
    case("tag 带后缀（v0.3.1-rc1）", tag_="v0.3.1-rc1", want="严格形如")
    case("DMG 仍在上传（state=starter）", lambda r, a: asset(a, ".dmg").__setitem__("state", "starter"), want="uploaded")
    case("setup.exe 没有 digest", lambda r, a: asset(a, "setup.exe").pop("digest"), want="digest")
    case("DMG 的 digest 为空", lambda r, a: asset(a, ".dmg").__setitem__("digest", ""), want="digest")
    case("digest 不是 sha256", lambda r, a: asset(a, ".dmg").__setitem__("digest", "sha1:" + "0" * 40), want="digest")
    case("size 为 0", lambda r, a: asset(a, ".dmg").__setitem__("size", 0), want="size")
    case("已发布（不是 draft）", lambda r, a: rel_of(r).__setitem__("draft", False), want="不是 draft")
    case("prerelease", lambda r, a: rel_of(r).__setitem__("prerelease", True), want="prerelease")
    case("同 tag 有两个 release", lambda r, a: r.append(dict(rel_of(r), id=1)), want="恰好一个，实际 2")
    case("还没有 draft", lambda r, a: r.remove(rel_of(r)), want="还没有 draft")
    case("缺便携 zip", lambda r, a: a.remove(asset(a, "portable.zip")), want="portable.zip 应恰好一个")
    case("缺 SHA256SUMS.txt", lambda r, a: a.remove(asset(a, SUMS_NAME)), want="SHA256SUMS.txt 应恰好一个")
    case("多一个未知资产", lambda r, a: a.append(dict(asset(a, SUMS_NAME), id=10, name="notes.txt")), want="清单之外")
    case("资产重名", lambda r, a: a.append(dict(asset(a, "setup.exe"), id=11)), want="setup.exe 应恰好一个")
    case("SHA256SUMS 内容被改过（与 digest 不符）", lambda r, a: base_sums + b"\n", want="与 GitHub digest")
    case("setup.exe 被替换（digest 与 SHA256SUMS 不符）",
         lambda r, a: asset(a, "setup.exe").__setitem__("digest", "sha256:" + "1" * 64), want="不是 attest 过的那个")

    def sums_with_dmg(r, a):
        extra = base_sums + b"%s  Clash.Assist_0.3.1_aarch64.dmg\n" % (b"2" * 64)
        asset(a, SUMS_NAME)["digest"] = "sha256:" + hashlib.sha256(extra).hexdigest()
        return extra
    case("SHA256SUMS 被追加 DMG 行", sums_with_dmg, want="应恰好列出")
    case("复读时 DMG 被替换", lambda r, a: asset(a, ".dmg").__setitem__("digest", "sha256:" + "3" * 64),
         baseline=lambda r, a: None, want="与第一次检查时不一致")
    case("复读时 release id 变了", lambda r, a: rel_of(r).__setitem__("id", 123), baseline=lambda r, a: None,
         want="release id")
    case("release 对象内的资产与资产列表不一致", lambda r, a: rel_of(r)["assets"].pop(), want="不一致")

    # 命令行：退出码与 summary
    with tempfile.TemporaryDirectory(prefix="ca-release-draft-") as tmp:
        td = Path(tmp)
        summary = td / "summary.md"
        argv = ["--tag", tag, "--releases", str(FIXTURES / "releases.json"), "--assets", str(FIXTURES / "assets.json"),
                "--sums", str(FIXTURES / "SHA256SUMS.txt"), "--summary", str(summary)]
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf), contextlib.redirect_stderr(buf):
            rc_ok = main(argv)
            rc_bad = main(["--tag", "v0.3.2"] + argv[2:])
            rc_missing = main(argv[:4] + ["--assets", str(td / "nope.json")] + argv[6:])
        s = summary.read_text(encoding="utf-8")
        for label, ok in [
            ("命令行通过时退出码 0", rc_ok == 0),
            ("命令行不通过时退出码 1", rc_bad == 1),
            ("文件不存在时退出码 2", rc_missing == 2),
            ("job summary 记下 DMG digest", "DMG digest" in s and asset(base_assets, ".dmg")["digest"] in s),
        ]:
            print(("ok   " if ok else "FAIL ") + label)
            fail |= 0 if ok else 1

    print("check-release-draft 自测：" + ("全部通过" if not fail else "失败"))
    return fail


def main(argv: list[str]) -> int:
    p = argparse.ArgumentParser(description="发布前检查 draft release（fail-closed）")
    p.add_argument("--self-test", action="store_true")
    p.add_argument("--tag")
    p.add_argument("--releases", type=Path)
    p.add_argument("--assets", type=Path)
    p.add_argument("--sums", type=Path)
    p.add_argument("--baseline-releases", type=Path)
    p.add_argument("--baseline-assets", type=Path)
    p.add_argument("--summary", type=Path)
    args = p.parse_args(argv)
    if args.self_test:
        return self_test()
    if not (args.tag and args.releases and args.assets and args.sums):
        print("check-release-draft：需要 --tag --releases --assets --sums（或 --self-test）", file=sys.stderr)
        return 2
    if (args.baseline_releases is None) != (args.baseline_assets is None):
        print("check-release-draft：--baseline-releases 与 --baseline-assets 必须同时给出", file=sys.stderr)
        return 2
    return run(args)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
