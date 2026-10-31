#!/usr/bin/env python3
"""发布 workflow 的结构检查（windows-plan r4 §2.8 / §3：M-6、S-5 ③、S-8 ⑥、S-R2）。

用 PyYAML 解析 .github/workflows/release-windows.yml，断言：
  1. 触发器只有 push（且只含 tags，每项以 v 开头）与 workflow_dispatch；
  2. workflow_dispatch 有 boolean 输入 dry_run，默认 true；
  3. 顶层 permissions 为 {}；每个 job 都显式声明 permissions；
  4. 含任何 write 权限的 job：`if` 必须同时要求 github.event_name == 'push' 与
     github.ref_type == 'tag'（且不含 `||`），并且不得 checkout 仓库代码（M-6）；
     这类 job 只能是 attest / release，权限必须与 WRITE_JOB_PERMISSIONS 完全一致；
     其余 job 的权限只能是 contents: read；
  4b. build job 在 tauri build 之后用 scan-nsis-rmdir.ps1 扫描生成的 installer.nsi，tag push 时 strict，
     不得带 if / continue-on-error（Ellie PR55 r2）；
  5. 每个 actions/checkout 都设置 persist-credentials: false（S-R2）；
  6. 不使用任何缓存：禁止 actions/cache，禁止 with.cache，setup-node 必须 package-manager-cache: false（S-8 ⑥）；
  4c. 冒烟诊断（M4c）：WER LocalDumps 只能在 build job 里配置；名为 smoke-diagnostics 的 artifact
     只能由 build job 上传，retention-days ≤ 7；
  7. 文本中不得出现 `--clobber`、`secrets.`（只允许 github.token）。

用法：python3 scripts/check-release-workflow.py [文件]   （默认 .github/workflows/release-windows.yml）
      python3 scripts/check-release-workflow.py --self-test   （对真实文件做变异，确认每条规则都能拦住）
退出码：0 = 通过；1 = 发现违规；2 = 用法错误 / 无法解析 / 缺少 PyYAML。
"""

import copy
import os
import sys

try:
    import yaml
except ImportError:  # pragma: no cover
    print("check-release-workflow: 需要 PyYAML（python3-yaml）", file=sys.stderr)
    sys.exit(2)

DEFAULT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".github", "workflows", "release-windows.yml")


# 有写权限的 job 及其完整权限集合（Ellie PR55：attest 补 contents: read；release 的
# gh attestation verify 只需 attestations: read）。多一项、少一项都算违规。
WRITE_JOB_PERMISSIONS = {
    "attest": {"contents": "read", "id-token": "write", "attestations": "write"},
    "release": {"contents": "write", "attestations": "read"},
}


def triggers(doc):
    # PyYAML 按 YAML 1.1 把键 on 解析成 True
    if "on" in doc:
        return doc["on"]
    return doc.get(True)


def is_false(v):
    return v is False or (isinstance(v, str) and v.strip().lower() == "false")


def check(doc, text):
    errs = []
    if not isinstance(doc, dict):
        return ["文档不是映射"]

    on = triggers(doc)
    if not isinstance(on, dict):
        errs.append("on 必须是映射")
        on = {}
    if set(on) != {"push", "workflow_dispatch"}:
        errs.append(f"触发器只能是 push 与 workflow_dispatch，实际：{sorted(map(str, on))}")
    push = on.get("push")
    if not isinstance(push, dict) or set(push) != {"tags"}:
        errs.append("push 只能含 tags（不得有 branches / paths 等）")
    else:
        tags = push["tags"]
        if not isinstance(tags, list) or not tags or not all(isinstance(t, str) and t.startswith("v") for t in tags):
            errs.append(f"push.tags 必须是以 v 开头的模式列表，实际：{tags!r}")
    wd = on.get("workflow_dispatch") or {}
    dry = ((wd.get("inputs") or {}) if isinstance(wd, dict) else {}).get("dry_run")
    if not isinstance(dry, dict) or dry.get("type") != "boolean" or dry.get("default") is not True:
        errs.append("workflow_dispatch 必须有 dry_run 输入（type: boolean，default: true）")

    if doc.get("permissions") != {}:
        errs.append(f"顶层 permissions 必须是 {{}}，实际：{doc.get('permissions')!r}")

    jobs = doc.get("jobs")
    if not isinstance(jobs, dict) or not jobs:
        errs.append("没有 jobs")
        jobs = {}
    for name, job in jobs.items():
        if not isinstance(job, dict):
            errs.append(f"job {name} 不是映射")
            continue
        perms = job.get("permissions")
        steps = job.get("steps") or []
        if not isinstance(perms, dict):
            errs.append(f"job {name} 必须显式声明 permissions 映射")
            perms = {}
        writes = sorted(k for k, v in perms.items() if v == "write")
        if writes:
            cond = str(job.get("if", ""))
            compact = cond.replace(" ", "").replace('"', "'")
            if ("github.event_name=='push'" not in compact or "github.ref_type=='tag'" not in compact
                    or "||" in cond):
                errs.append(f"job {name} 有写权限 {writes}，但 if 未限定为 tag push：{cond!r}")
            want = WRITE_JOB_PERMISSIONS.get(name)
            if want is None:
                errs.append(f"job {name} 有写权限 {writes}，但只有 {sorted(WRITE_JOB_PERMISSIONS)} 可以有写权限")
            elif perms != want:
                errs.append(f"job {name} 的权限必须恰为 {want!r}，实际：{perms!r}")
            for s in steps:
                if isinstance(s, dict) and str(s.get("uses", "")).startswith("actions/checkout@"):
                    errs.append(f"job {name} 有写权限，不得 checkout 仓库代码（M-6）")
        elif perms != {"contents": "read"}:
            errs.append(f"job {name} 无需写权限，permissions 只能是 contents: read，实际：{perms!r}")

        for i, s in enumerate(steps):
            if not isinstance(s, dict):
                continue
            uses = str(s.get("uses", ""))
            w = s.get("with") or {}
            where = f"job {name} step {i + 1}（{s.get('name') or uses or 'run'}）"
            if uses.startswith("actions/checkout@") and not is_false(w.get("persist-credentials")):
                errs.append(f"{where}：checkout 必须 persist-credentials: false")
            if uses.startswith("actions/cache"):
                errs.append(f"{where}：发布 workflow 不得使用缓存")
            if isinstance(w, dict) and "cache" in w:
                errs.append(f"{where}：发布 workflow 不得设置 cache")
            if uses.startswith("actions/setup-node@") and not is_false(w.get("package-manager-cache")):
                errs.append(f"{where}：setup-node 必须 package-manager-cache: false")

    # Ellie PR55 r2：build job 必须在 tauri build 之后扫描生成的 installer.nsi，tag push 时 strict。
    build_steps = [x for x in ((jobs.get("build") or {}).get("steps") or []) if isinstance(x, dict)]
    runs = [str(x.get("run", "")) for x in build_steps]
    tauri_at = next((i for i, r in enumerate(runs) if "tauri build" in r), None)
    scan_at = next((i for i, r in enumerate(runs) if "scan-nsis-rmdir.ps1" in r), None)
    if scan_at is None:
        errs.append("build job 缺少 installer.nsi 的 RmDir /r 扫描步骤（scan-nsis-rmdir.ps1）")
    else:
        st = build_steps[scan_at]
        r = runs[scan_at].replace(" ", "")
        if tauri_at is None or scan_at < tauri_at:
            errs.append("installer.nsi 扫描必须在 tauri build 之后")
        if "release/nsis/x64/installer.nsi" not in r:
            errs.append("installer.nsi 扫描必须扫 release/nsis/x64/installer.nsi")
        if "Include=@('src-tauri/windows/hooks.nsh')" not in r:
            errs.append("installer.nsi 扫描必须同时扫 hooks.nsh（Include = @('src-tauri/windows/hooks.nsh')）")
        if "Strict=($env:EVENT_NAME-eq'push')" not in r:
            errs.append("installer.nsi 扫描必须在 tag push 时 strict（Strict = ($env:EVENT_NAME -eq 'push')）")
        if "exit$LASTEXITCODE" not in r:
            errs.append("installer.nsi 扫描必须以 exit $LASTEXITCODE 结束")
        if "if" in st or st.get("continue-on-error"):
            errs.append("installer.nsi 扫描步骤不得有 if / continue-on-error")

    # M4c：冒烟诊断只在 build job（contents: read、一次性 runner），诊断 artifact 短期保留。
    for name, job in jobs.items():
        if not isinstance(job, dict):
            continue
        for x in job.get("steps") or []:
            if not isinstance(x, dict):
                continue
            if "LocalDumps" in str(x.get("run", "")) and name != "build":
                errs.append(f"job {name}：WER LocalDumps 只能在 build job 中配置")
            w = x.get("with") or {}
            if str(x.get("uses", "")).startswith("actions/upload-artifact@") and isinstance(w, dict) and w.get("name") == "smoke-diagnostics":
                if name != "build":
                    errs.append(f"job {name}：smoke-diagnostics 只能由 build job 上传")
                rd = w.get("retention-days")
                if not isinstance(rd, int) or not 1 <= rd <= 7:
                    errs.append(f"smoke-diagnostics 的 retention-days 必须是 1–7 的整数，实际：{rd!r}")

    if "--clobber" in text:
        errs.append("不得使用 --clobber（不得覆盖已有资产）")
    if "secrets." in text:
        errs.append("不得引用 secrets.*（只允许 github.token）")
    return errs


def load(path):
    try:
        with open(path, encoding="utf-8") as f:
            text = f.read()
        return yaml.safe_load(text), text
    except (OSError, yaml.YAMLError) as e:
        print(f"check-release-workflow: 无法读取 {path}：{e}", file=sys.stderr)
        sys.exit(2)


def self_test(path):
    doc, text = load(path)
    base = check(doc, text)
    if base:
        print("FAIL 真实文件本身未通过：")
        for e in base:
            print("  " + e)
        return 1

    def job_with_write(d):
        return next(n for n, j in d["jobs"].items() if "write" in (j.get("permissions") or {}).values())

    def first_checkout(d):
        for j in d["jobs"].values():
            for s in j.get("steps") or []:
                if str(s.get("uses", "")).startswith("actions/checkout@"):
                    return s
        raise AssertionError("没有 checkout")

    def first_setup_node(d):
        for j in d["jobs"].values():
            for s in j.get("steps") or []:
                if str(s.get("uses", "")).startswith("actions/setup-node@"):
                    return s
        raise AssertionError("没有 setup-node")

    def nsi_step(d):
        return next(x for x in d["jobs"]["build"]["steps"] if "scan-nsis-rmdir" in str(x.get("run", "")))

    def diag_upload(d):
        return next(x for x in d["jobs"]["build"]["steps"] if (x.get("with") or {}).get("name") == "smoke-diagnostics")

    def dumps_step(d):
        return next(x for x in d["jobs"]["build"]["steps"] if "LocalDumps" in str(x.get("run", "")))

    def move_nsi_first(d):
        st = nsi_step(d)
        d["jobs"]["build"]["steps"].remove(st)
        d["jobs"]["build"]["steps"].insert(0, st)

    def set_on(d, k, v):
        triggers(d)[k] = v

    mutations = [
        ("额外触发器 pull_request", lambda d, t: (set_on(d, "pull_request", None), t), "触发器只能是"),
        ("push.branches", lambda d, t: (triggers(d)["push"].__setitem__("branches", ["main"]), t), "push 只能含 tags"),
        ("tag 模式不以 v 开头", lambda d, t: (triggers(d)["push"].__setitem__("tags", ["*"]), t), "push.tags"),
        ("dry_run 默认 false", lambda d, t: (triggers(d)["workflow_dispatch"]["inputs"]["dry_run"].__setitem__("default", False), t), "dry_run"),
        ("缺少 dry_run", lambda d, t: (triggers(d).__setitem__("workflow_dispatch", None), t), "dry_run"),
        ("顶层 contents: write", lambda d, t: (d.__setitem__("permissions", {"contents": "write"}), t), "顶层 permissions"),
        ("build 拿到 contents: write", lambda d, t: (d["jobs"]["build"].__setitem__("permissions", {"contents": "write"}), t), "if 未限定"),
        ("build 多了 id-token: read", lambda d, t: (d["jobs"]["build"]["permissions"].__setitem__("id-token", "read"), t), "只能是 contents: read"),
        ("attest 缺 contents: read", lambda d, t: (d["jobs"]["attest"]["permissions"].pop("contents"), t), "权限必须恰为"),
        ("release 多了 id-token: write", lambda d, t: (d["jobs"]["release"]["permissions"].__setitem__("id-token", "write"), t), "权限必须恰为"),
        ("release 的 attestations 升为 write", lambda d, t: (d["jobs"]["release"]["permissions"].__setitem__("attestations", "write"), t), "权限必须恰为"),
        ("新 job 拿写权限", lambda d, t: (d["jobs"].__setitem__("extra", {"if": "github.event_name == 'push' && github.ref_type == 'tag'", "permissions": {"contents": "write"}, "steps": []}), t), "只有"),
        ("写权限 job 去掉 if", lambda d, t: (d["jobs"][job_with_write(d)].pop("if"), t), "if 未限定"),
        ("写权限 job 的 if 放行 dispatch", lambda d, t: (d["jobs"][job_with_write(d)].__setitem__("if", "github.event_name == 'push' && github.ref_type == 'tag' || inputs.dry_run == false"), t), "if 未限定"),
        ("写权限 job checkout", lambda d, t: (d["jobs"][job_with_write(d)]["steps"].insert(0, {"uses": "actions/checkout@" + "0" * 40, "with": {"persist-credentials": False}}), t), "不得 checkout"),
        ("checkout 持久化凭据", lambda d, t: (first_checkout(d)["with"].pop("persist-credentials"), t), "persist-credentials"),
        ("setup-node cache", lambda d, t: (first_setup_node(d)["with"].__setitem__("cache", "pnpm"), t), "不得设置 cache"),
        ("setup-node 自动缓存", lambda d, t: (first_setup_node(d)["with"].pop("package-manager-cache"), t), "package-manager-cache"),
        ("actions/cache", lambda d, t: (d["jobs"]["build"]["steps"].insert(1, {"uses": "actions/cache@" + "0" * 40, "with": {"path": "x"}}), t), "不得使用缓存"),
        ("删掉 installer.nsi 扫描", lambda d, t: (d["jobs"]["build"].__setitem__("steps", [x for x in d["jobs"]["build"]["steps"] if "scan-nsis-rmdir" not in str(x.get("run", ""))]), t), "缺少 installer.nsi"),
        ("installer.nsi 扫描不 strict", lambda d, t: (nsi_step(d).__setitem__("run", nsi_step(d)["run"].replace("Strict = ($env:EVENT_NAME -eq 'push')", "Strict = $false")), t), "strict"),
        ("installer.nsi 扫描不带 hooks.nsh", lambda d, t: (nsi_step(d).__setitem__("run", nsi_step(d)["run"].replace(" Include = @('src-tauri/windows/hooks.nsh');", "")), t), "hooks.nsh"),
        ("installer.nsi 扫描 continue-on-error", lambda d, t: (nsi_step(d).__setitem__("continue-on-error", True), t), "continue-on-error"),
        ("installer.nsi 扫描加 if", lambda d, t: (nsi_step(d).__setitem__("if", "false"), t), "不得有 if"),
        ("installer.nsi 扫描挪到 tauri build 之前", lambda d, t: (move_nsi_first(d), t), "之后"),
        ("smoke-diagnostics 保留 30 天", lambda d, t: (diag_upload(d)["with"].__setitem__("retention-days", 30), t), "retention-days"),
        ("smoke-diagnostics 保留期是表达式", lambda d, t: (diag_upload(d)["with"].__setitem__("retention-days", "${{ 3 }}"), t), "retention-days"),
        ("release job 配 LocalDumps", lambda d, t: (d["jobs"]["release"]["steps"].append(copy.deepcopy(dumps_step(d))), t), "LocalDumps"),
        ("release job 上传 smoke-diagnostics", lambda d, t: (d["jobs"]["release"]["steps"].append(copy.deepcopy(diag_upload(d))), t), "只能由 build job"),
        ("--clobber", lambda d, t: (None, t + "\n# gh release upload --clobber\n"), "--clobber"),
        ("secrets.", lambda d, t: (None, t + "\n# ${{ secrets.TOKEN }}\n"), "secrets"),
    ]
    fail = 0
    for label, mut, want in mutations:
        d = copy.deepcopy(doc)
        _, t = mut(d, text)
        errs = check(d, t)
        if any(want in e for e in errs):
            print(f"ok   {label} 被拒绝")
        else:
            print(f"FAIL {label} 未被拒绝（期望包含 {want!r}）：{errs}")
            fail = 1
    print("check-release-workflow 自测：" + ("全部通过" if not fail else "失败"))
    return fail


def main(argv):
    args = argv[1:]
    if args[:1] == ["--self-test"]:
        if len(args) > 2:
            print("用法：check-release-workflow.py --self-test [文件]", file=sys.stderr)
            return 2
        return self_test(args[1] if len(args) == 2 else DEFAULT)
    if len(args) > 1 or (args and args[0].startswith("-")):
        print("用法：check-release-workflow.py [文件] | --self-test [文件]", file=sys.stderr)
        return 2
    path = args[0] if args else DEFAULT
    doc, text = load(path)
    errs = check(doc, text)
    for e in errs:
        print(f"{os.path.relpath(path)}: {e}")
    if errs:
        return 1
    print(f"check-release-workflow: 通过（{os.path.relpath(path)}）")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
