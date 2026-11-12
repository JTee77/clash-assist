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
  4d. build job 在 tauri build 之后用 scan-exe-markers.ps1 扫描 release exe，构建类型必须是 -Build Release
     （M5：FAKE_LATEST 与 updater-e2e 标记出现即失败），不得带 if / continue-on-error；
     任何 run 都不得带 `--features updater-e2e`（M-7）；
  5. 每个 actions/checkout 都设置 persist-credentials: false（S-R2）；
  6. 不使用任何缓存：禁止 actions/cache，禁止 with.cache，setup-node 必须 package-manager-cache: false（S-8 ⑥）；
  4c. 冒烟诊断（M4c）：WER LocalDumps 只能在 build job 里配置；名为 smoke-diagnostics 的 artifact
     只能由 build job 上传，retention-days ≤ 7；
     M4d：smoke-diagnostics 的 path 必须排除 dumps/** 与 **/*.dmp（仓库公开，常规 artifact 不得含内存转储）；
     minidump 只能由 smoke-crash-dumps 上传，其 if 必须是 failure() && steps.smoke.outcome == 'failure'
     （id: smoke 为冒烟测试步骤），retention-days ≤ 7；
  7. 文本中不得出现 `--clobber`、`secrets.`（只允许 github.token）。

另外检查 .github/workflows/publish-release.yml（0.3.1 发布前闸门，规则 P1–P8，见 check_publish）：
  P1. 只有 workflow_dispatch 触发，且只有一个必填的 string 输入 tag；
  P2. 顶层 permissions 为 {}；只有一个 job，environment 为 release，permissions 恰为 contents: write，
      job 与所有步骤都不得带 if / continue-on-error；concurrency 按 inputs.tag 分组且不取消进行中的运行；
  P3. 所有 uses 固定到 40 位 commit SHA；checkout 必须 persist-credentials: false；不得使用缓存；
  P4. run 里不得出现 ${{ }}（输入一律经 env 传入）；job 级 env 不得带 GH_TOKEN（令牌只给需要的步骤）；
  P5. 先校验 tag 与运行 ref、版本一致性（check-version-sync.mjs --tag）、tag 位于 main；
      check-release-draft.py 恰好跑两次（第二次带 --baseline-*），两次都带 --platforms，这两步不拿令牌；
      release/platforms.json 必须按 ref=$GITHUB_SHA 经 API 读取，且排在两次检查之前（#71 平台改动标记）；
  P6. 恰好一个发布步骤（含 draft=false），是最后一步，排在上述所有检查之后；
      整个文件只有这一处 PATCH，且只带 draft=false 一个字段，目标是 releases/$id；
  P7. 不得有上传 / 删除 / 新建 / 编辑资产或 release 的命令（gh release upload/delete/create/edit、
      DELETE、POST、PUT、uploads.github.com、--clobber），不得重新计算 SHA256SUMS（sha256sum），
      不得引用 secrets.*。

用法：python3 scripts/check-release-workflow.py [文件]   （默认同时检查 release-windows.yml 与 publish-release.yml；
                                                         文件名为 publish-release.yml 时按 P 规则检查）
      python3 scripts/check-release-workflow.py --self-test [文件]   （对真实文件做变异，确认每条规则都能拦住）
退出码：0 = 通过；1 = 发现违规；2 = 用法错误 / 无法解析 / 缺少 PyYAML。
"""

import copy
import os
import re
import sys

try:
    import yaml
except ImportError:  # pragma: no cover
    print("check-release-workflow: 需要 PyYAML（python3-yaml）", file=sys.stderr)
    sys.exit(2)

DEFAULT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".github", "workflows", "release-windows.yml")
DEFAULT_PUBLISH = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".github", "workflows", "publish-release.yml")


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

    # M5（M-7 / S-R4）：release exe 的测试标记扫描必须是 -Build Release，且在 tauri build 之后。
    exe_at = next((i for i, r in enumerate(runs) if "scan-exe-markers.ps1" in r), None)
    if exe_at is None:
        errs.append("build job 缺少 exe 测试标记扫描步骤（scan-exe-markers.ps1）")
    else:
        st = build_steps[exe_at]
        r = " ".join(runs[exe_at].split())
        if tauri_at is None or exe_at < tauri_at:
            errs.append("exe 测试标记扫描必须在 tauri build 之后")
        if "/release/clash-assist.exe" not in r:
            errs.append("exe 测试标记扫描必须扫 release/clash-assist.exe")
        if "-Build Release" not in r or "-Build Debug" in r or "-Build E2E" in r:
            errs.append("exe 测试标记扫描必须是 -Build Release")
        if "exit $LASTEXITCODE" not in r:
            errs.append("exe 测试标记扫描必须以 exit $LASTEXITCODE 结束")
        if "if" in st or st.get("continue-on-error"):
            errs.append("exe 测试标记扫描步骤不得有 if / continue-on-error")
    if "updater-e2e" in "\n".join(str(x.get("run", "")) for j in jobs.values() if isinstance(j, dict) for x in (j.get("steps") or []) if isinstance(x, dict)):
        errs.append("发布 workflow 不得启用 updater-e2e feature（M-7）")

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
                lines = [l.strip() for l in str(w.get("path", "")).splitlines() if l.strip()]
                if not any(l.startswith("!") and l.endswith("smoke-diag/dumps/**") for l in lines) or not any(
                    l.startswith("!") and l.endswith("smoke-diag/**/*.dmp") for l in lines
                ):
                    errs.append("smoke-diagnostics 的 path 必须排除 smoke-diag/dumps/** 与 smoke-diag/**/*.dmp（常规 artifact 不得含内存转储）")
            if str(x.get("uses", "")).startswith("actions/upload-artifact@") and isinstance(w, dict) and w.get("name") != "smoke-diagnostics":
                path = str(w.get("path", ""))
                if ("dumps" in path or ".dmp" in path or "smoke-diag" in path) and w.get("name") != "smoke-crash-dumps":
                    errs.append(f"job {name}：artifact {w.get('name')!r} 不得上传冒烟诊断 / minidump（只能由 smoke-crash-dumps 上传）")
                if w.get("name") == "smoke-crash-dumps":
                    if name != "build":
                        errs.append(f"job {name}：smoke-crash-dumps 只能由 build job 上传")
                    cond = str(x.get("if", "")).replace(" ", "").replace('"', "'")
                    if cond.startswith("${{") and cond.endswith("}}"):
                        cond = cond[3:-2]
                    if cond != "failure()&&steps.smoke.outcome=='failure'":
                        errs.append(f"smoke-crash-dumps 只能在冒烟失败时上传（if: failure() && steps.smoke.outcome == 'failure'），实际：{x.get('if')!r}")
                    rd = w.get("retention-days")
                    if not isinstance(rd, int) or not 1 <= rd <= 7:
                        errs.append(f"smoke-crash-dumps 的 retention-days 必须是 1–7 的整数，实际：{rd!r}")
    smoke = [x for x in build_steps if "smoke-windows.ps1" in str(x.get("run", ""))]
    if len(smoke) != 1 or smoke[0].get("id") != "smoke":
        errs.append("build job 必须恰有一个冒烟测试步骤（smoke-windows.ps1），且 id 为 smoke")

    if "--clobber" in text:
        errs.append("不得使用 --clobber（不得覆盖已有资产）")
    if "secrets." in text:
        errs.append("不得引用 secrets.*（只允许 github.token）")
    return errs


# ---------------------------------------------------------------------------
# publish-release.yml（发布前闸门）
# ---------------------------------------------------------------------------

PIN_RE = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(/[^@\s]+)?@[0-9a-f]{40}")
PUBLISH_FORBIDDEN = [
    (re.compile(r"\bgh\s+release\s+(upload|delete|delete-asset|create|edit)\b"), "gh release upload/delete/create/edit"),
    (re.compile(r"(-X|--method)\s*['\"]?(DELETE|POST|PUT)\b", re.I), "DELETE / POST / PUT 请求"),
    (re.compile(r"uploads\.github\.com"), "uploads.github.com（上传资产）"),
    (re.compile(r"--clobber"), "--clobber"),
    (re.compile(r"\bsha256sum\b|Get-FileHash"), "重新计算 SHA256SUMS"),
    (re.compile(r"secrets\."), "secrets.*（只允许 github.token）"),
]
PATCH_RE = re.compile(r"(-X|--method)\s*['\"]?PATCH\b", re.I)


def _env_has_token(env):
    return isinstance(env, dict) and any(k in ("GH_TOKEN", "GITHUB_TOKEN") for k in env)


def check_publish(doc, text):
    errs = []
    if not isinstance(doc, dict):
        return ["文档不是映射"]

    # P1 触发器
    on = triggers(doc)
    if not isinstance(on, dict) or set(on) != {"workflow_dispatch"}:
        errs.append(f"P1 触发器只能是 workflow_dispatch，实际：{sorted(map(str, on)) if isinstance(on, dict) else on!r}")
        on = on if isinstance(on, dict) else {}
    wd = on.get("workflow_dispatch") if isinstance(on.get("workflow_dispatch"), dict) else {}
    inputs = wd.get("inputs") if isinstance(wd.get("inputs"), dict) else {}
    tag_in = inputs.get("tag")
    if set(inputs) != {"tag"} or not isinstance(tag_in, dict) or tag_in.get("required") is not True or tag_in.get("type") != "string":
        errs.append(f"P1 workflow_dispatch 只能有一个输入 tag（type: string，required: true），实际：{inputs!r}")

    # P2 权限 / environment / job 结构 / concurrency
    if doc.get("permissions") != {}:
        errs.append(f"P2 顶层 permissions 必须是 {{}}，实际：{doc.get('permissions')!r}")
    conc = doc.get("concurrency")
    if not isinstance(conc, dict) or "inputs.tag" not in str(conc.get("group", "")) or conc.get("cancel-in-progress") is not False:
        errs.append(f"P2 concurrency 必须按 inputs.tag 分组且 cancel-in-progress: false，实际：{conc!r}")
    jobs = doc.get("jobs") if isinstance(doc.get("jobs"), dict) else {}
    if len(jobs) != 1:
        errs.append(f"P2 只能有一个 job（检查与发布在同一 job），实际：{sorted(jobs)}")
    for name, job in jobs.items():
        if not isinstance(job, dict):
            errs.append(f"P2 job {name} 不是映射")
            continue
        envname = job.get("environment")
        if isinstance(envname, dict):
            envname = envname.get("name")
        if envname != "release":
            errs.append(f"P2 job {name} 必须 environment: release（用户审批），实际：{job.get('environment')!r}")
        if job.get("permissions") != {"contents": "write"}:
            errs.append(f"P2 job {name} 的 permissions 必须恰为 contents: write，实际：{job.get('permissions')!r}")
        if "if" in job or job.get("continue-on-error"):
            errs.append(f"P2 job {name} 不得有 if / continue-on-error")
        if _env_has_token(job.get("env")):
            errs.append(f"P4 job {name} 的 job 级 env 不得带 GH_TOKEN（令牌只给需要的步骤）")
        steps = [x for x in (job.get("steps") or []) if isinstance(x, dict)]
        runs = [str(x.get("run", "")) for x in steps]
        for i, st in enumerate(steps):
            uses = str(st.get("uses", ""))
            w = st.get("with") or {}
            where = f"job {name} step {i + 1}（{st.get('name') or uses or 'run'}）"
            if "if" in st or st.get("continue-on-error"):
                errs.append(f"P2 {where}：不得有 if / continue-on-error（任何检查失败都必须终止）")
            if uses:
                ref = uses.split("#")[0].strip()
                if not (ref.startswith("./") or PIN_RE.fullmatch(ref)):
                    errs.append(f"P3 {where}：uses 必须固定到 40 位 commit SHA，实际：{uses!r}")
            if uses.startswith("actions/checkout@") and not is_false((w if isinstance(w, dict) else {}).get("persist-credentials")):
                errs.append(f"P3 {where}：checkout 必须 persist-credentials: false")
            if uses.startswith("actions/cache") or (isinstance(w, dict) and "cache" in w):
                errs.append(f"P3 {where}：不得使用缓存")
            if "${{" in runs[i]:
                errs.append(f"P4 {where}：run 里不得出现 ${{{{ }}}}，输入须经 env 传入")

        # P5 检查步骤
        def idx(pred):
            return [i for i, r in enumerate(runs) if pred(r)]
        tag_chk = idx(lambda r: 'refs/tags/$TAG' in r and "GITHUB_REF" in r and "=~" in r)
        ver_chk = idx(lambda r: "check-version-sync.mjs" in r and "--tag" in r)
        main_chk = idx(lambda r: "compare/main..." in r)
        gates = idx(lambda r: "check-release-draft.py" in r)
        if not tag_chk:
            errs.append("P5 缺少 tag 格式与运行 ref（refs/tags/$TAG）检查")
        if not ver_chk:
            errs.append("P5 缺少版本一致性检查（check-version-sync.mjs --tag）")
        if not main_chk:
            errs.append("P5 缺少 tag 位于 main 的检查（compare/main...）")
        first_gate = [i for i in gates if "--baseline" not in runs[i]]
        reread_gate = [i for i in gates if "--baseline-releases" in runs[i] and "--baseline-assets" in runs[i]]
        if len(gates) != 2 or len(first_gate) != 1 or len(reread_gate) != 1 or first_gate[0] > reread_gate[0]:
            errs.append("P5 check-release-draft.py 必须恰好跑两次：先检查，复读后再带 --baseline-* 检查一次")
        plat_fetch = idx(lambda r: "contents/release/platforms.json?ref=$GITHUB_SHA" in r)
        if len(plat_fetch) != 1:
            errs.append("P5 必须恰好一处按 ref=$GITHUB_SHA 经 API 读取 release/platforms.json")
        elif gates and plat_fetch[0] > min(gates):
            errs.append("P5 release/platforms.json 必须在检查之前读取")
        for i in gates:
            if "--platforms" not in runs[i]:
                errs.append("P5 检查脚本必须带 --platforms（核对平台改动标记）")
            if _env_has_token(steps[i].get("env")):
                errs.append("P5 检查脚本步骤不得拿令牌（只读快照文件）")
            if "|| true" in runs[i] or "; true" in runs[i]:
                errs.append("P5 检查脚本的失败不得被吞掉")
        if reread_gate:
            fetch_between = [i for i in range(first_gate[0] + 1 if first_gate else 0, reread_gate[0]) if "repos/$REPO/releases" in runs[i]]
            if not fetch_between:
                errs.append("P5 两次检查之间必须重新读取 release（复读，缩小竞态窗口）")

        # P6 发布步骤
        pub = idx(lambda r: "draft=false" in r)
        if len(pub) != 1:
            errs.append(f"P6 必须恰好有一个发布步骤（draft=false），实际 {len(pub)} 个")
        else:
            pi = pub[0]
            if pi != len(steps) - 1:
                errs.append("P6 发布步骤必须是最后一步")
            before = tag_chk + ver_chk + main_chk + gates
            if not before or any(i > pi for i in before):
                errs.append("P6 发布步骤必须排在所有检查之后")
            r = runs[pi]
            patch_lines = [l for l in r.splitlines() if PATCH_RE.search(l)]
            if len(patch_lines) != 1:
                errs.append(f"P6 发布步骤必须恰好一处 PATCH，实际 {len(patch_lines)} 处")
            else:
                line = patch_lines[0]
                fields = re.findall(r"(?:^|\s)(?:-[fF]|--field|--raw-field|--input)[\s=]+(\S+)", line)
                if fields != ["draft=false"]:
                    errs.append(f"P6 PATCH 只能带 draft=false 一个字段，实际：{fields}")
                if not re.search(r'repos/\$REPO/releases/\$id["\s]', line):
                    errs.append("P6 PATCH 的目标必须是 repos/$REPO/releases/$id")
    if len(PATCH_RE.findall(text)) != 1:
        errs.append(f"P6 整个文件只能有一处 PATCH（发布），实际 {len(PATCH_RE.findall(text))} 处")

    # P7 禁止写资产 / 改 release 的命令
    for rx, what in PUBLISH_FORBIDDEN:
        if rx.search(text):
            errs.append(f"P7 不得出现 {what}")
    return errs


def is_publish(path):
    return os.path.basename(path) in ("publish-release.yml", "publish-release.yaml")


def self_test_publish(path):
    doc, text = load(path)
    base = check_publish(doc, text)
    if base:
        print("FAIL publish-release 真实文件本身未通过：")
        for e in base:
            print("  " + e)
        return 1

    def job(d):
        return next(iter(d["jobs"].values()))

    def steps(d):
        return job(d)["steps"]

    def step_with(d, needle, nth=0):
        return [x for x in steps(d) if needle in str(x.get("run", ""))][nth]

    def pub(d):
        return step_with(d, "draft=false")

    def replace_run(st, a, b):
        assert a in st["run"], a
        st["run"] = st["run"].replace(a, b)

    def text_replace(t, a, b):
        assert a in t, a
        return t.replace(a, b)

    def move_pub_first(d):
        st = pub(d)
        steps(d).remove(st)
        steps(d).insert(0, st)

    def drop(d, needle, nth=0):
        steps(d).remove(step_with(d, needle, nth))

    def set_on(d, k, v):
        triggers(d)[k] = v

    mutations = [
        ("加 push 触发", lambda d, t: (set_on(d, "push", {"tags": ["v*"]}), t), "P1 触发器"),
        ("加 release 触发", lambda d, t: (set_on(d, "release", {"types": ["created"]}), t), "P1 触发器"),
        ("tag 输入非必填", lambda d, t: (triggers(d)["workflow_dispatch"]["inputs"]["tag"].__setitem__("required", False), t), "P1 workflow_dispatch"),
        ("多一个输入", lambda d, t: (triggers(d)["workflow_dispatch"]["inputs"].__setitem__("force", {"type": "boolean"}), t), "P1 workflow_dispatch"),
        ("顶层 contents: write", lambda d, t: (d.__setitem__("permissions", {"contents": "write"}), t), "P2 顶层"),
        ("去掉 environment", lambda d, t: (job(d).pop("environment"), t), "environment: release"),
        ("environment 改名", lambda d, t: (job(d).__setitem__("environment", "prod"), t), "environment: release"),
        ("多了 actions: write", lambda d, t: (job(d)["permissions"].__setitem__("actions", "write"), t), "恰为 contents: write"),
        ("多一个 job", lambda d, t: (d["jobs"].__setitem__("extra", {"runs-on": "x", "permissions": {}, "steps": []}), t), "只能有一个 job"),
        ("job 加 if", lambda d, t: (job(d).__setitem__("if", "always()"), t), "不得有 if"),
        ("检查步骤加 if", lambda d, t: (step_with(d, "check-release-draft.py").__setitem__("if", "false"), t), "不得有 if"),
        ("检查步骤 continue-on-error", lambda d, t: (step_with(d, "check-release-draft.py").__setitem__("continue-on-error", True), t), "continue-on-error"),
        ("检查失败被吞掉", lambda d, t: (replace_run(step_with(d, "check-release-draft.py"), '--summary "$GITHUB_STEP_SUMMARY"', '--summary "$GITHUB_STEP_SUMMARY" || true'), t), "吞掉"),
        ("concurrency 不按 tag", lambda d, t: (d["concurrency"].__setitem__("group", "publish"), t), "concurrency"),
        ("concurrency 取消进行中", lambda d, t: (d["concurrency"].__setitem__("cancel-in-progress", True), t), "concurrency"),
        ("action 用 tag 而非 SHA", lambda d, t: (steps(d)[1].__setitem__("uses", "actions/checkout@v4"), t), "40 位"),
        ("checkout 持久化凭据", lambda d, t: (steps(d)[1]["with"].pop("persist-credentials"), t), "persist-credentials"),
        ("run 里内联 inputs.tag", lambda d, t: (replace_run(step_with(d, "check-version-sync.mjs"), '"$TAG"', '"${{ inputs.tag }}"'), t), "P4"),
        ("job 级 GH_TOKEN", lambda d, t: (job(d)["env"].__setitem__("GH_TOKEN", "${{ github.token }}"), t), "job 级 env"),
        ("检查步骤拿令牌", lambda d, t: (step_with(d, "check-release-draft.py").__setitem__("env", {"GH_TOKEN": "x"}), t), "不得拿令牌"),
        ("删掉 tag/ref 检查", lambda d, t: (drop(d, "refs/tags/$TAG"), t), "运行 ref"),
        ("删掉版本一致性", lambda d, t: (drop(d, "check-version-sync.mjs"), t), "版本一致性"),
        ("删掉 tag 在 main 的检查", lambda d, t: (drop(d, "compare/main..."), t), "位于 main"),
        ("删掉第一次检查", lambda d, t: (steps(d).remove([x for x in steps(d) if "check-release-draft.py" in str(x.get("run", "")) and "--baseline" not in x["run"]][0]), t), "恰好跑两次"),
        ("删掉复读后的检查", lambda d, t: (drop(d, "--baseline-releases"), t), "恰好跑两次"),
        ("复读检查不带 baseline", lambda d, t: (replace_run(step_with(d, "--baseline-releases"), "--baseline-releases", "--x"), t), "恰好跑两次"),
        ("检查不带 --platforms", lambda d, t: (replace_run(step_with(d, "--baseline-releases"), '--platforms "$RUNNER_TEMP/platforms.json"', ""), t), "--platforms"),
        ("platforms.json 不按提交读取", lambda d, t: (replace_run(step_with(d, "contents/release/platforms.json"), "?ref=$GITHUB_SHA", "?ref=main"), t), "ref=$GITHUB_SHA"),
        ("删掉 platforms.json 读取", lambda d, t: (drop(d, "contents/release/platforms.json"), t), "ref=$GITHUB_SHA"),
        ("platforms.json 读取挪到检查之后", lambda d, t: (steps(d).insert(len(steps(d)) - 1, steps(d).pop(steps(d).index(step_with(d, "contents/release/platforms.json")))), t), "之前读取"),
        ("删掉复读", lambda d, t: (drop(d, "snap2\"; mkdir"), t), "复读"),
        ("发布挪到最前", lambda d, t: (move_pub_first(d), t), "P6 发布步骤"),
        ("发布后再加一步", lambda d, t: (steps(d).append({"name": "x", "run": "echo hi"}), t), "最后一步"),
        ("两个发布步骤", lambda d, t: (steps(d).append(copy.deepcopy(pub(d))), t), "恰好有一个发布步骤"),
        ("PATCH 多带字段", lambda d, t: (replace_run(pub(d), "-F draft=false", "-F draft=false -f body=x"), t), "只能带 draft=false"),
        ("PATCH 改 tag_name", lambda d, t: (replace_run(pub(d), "-F draft=false", "-F draft=false -f tag_name=v9.9.9"), t), "只能带 draft=false"),
        ("PATCH 打到资产上", lambda d, t: (replace_run(pub(d), 'releases/$id"', 'releases/assets/$id"'), t), "目标必须是"),
        ("文件里另有 PATCH", lambda d, t: (None, t + "\n# gh api -X PATCH repos/x/releases/assets/1 -f name=y\n"), "只能有一处 PATCH"),
        ("gh release upload", lambda d, t: (None, t + "\n# gh release upload v1 x.dmg\n"), "P7"),
        ("gh release edit", lambda d, t: (None, t + "\n# gh release edit v1 --draft=false\n"), "P7"),
        ("gh release delete-asset", lambda d, t: (None, t + "\n# gh release delete-asset v1 x\n"), "P7"),
        ("DELETE 资产", lambda d, t: (None, t + "\n# gh api -X DELETE repos/x/releases/assets/1\n"), "P7"),
        ("POST 上传", lambda d, t: (None, t + "\n# curl -X POST https://uploads.github.com/x\n"), "P7"),
        ("重算 SHA256SUMS", lambda d, t: (None, t + "\n# sha256sum *.dmg >> SHA256SUMS.txt\n"), "重新计算"),
        ("--clobber", lambda d, t: (None, t + "\n# --clobber\n"), "--clobber"),
        ("secrets.", lambda d, t: (None, t + "\n# ${{ secrets.PAT }}\n"), "secrets"),
    ]
    fail = 0
    for label, mut, want in mutations:
        d = copy.deepcopy(doc)
        _, t = mut(d, text)
        errs = check_publish(d, t)
        if any(want in e for e in errs):
            print(f"ok   publish-release：{label} 被拒绝")
        else:
            print(f"FAIL publish-release：{label} 未被拒绝（期望包含 {want!r}）：{errs}")
            fail = 1
    print("check-release-workflow 自测（publish-release）：" + ("全部通过" if not fail else "失败"))
    return fail


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

    def dumps_upload(d):
        return next(x for x in d["jobs"]["build"]["steps"] if (x.get("with") or {}).get("name") == "smoke-crash-dumps")

    def drop_dmp_exclusion(d):
        w = diag_upload(d)["with"]
        w["path"] = "\n".join(l for l in w["path"].splitlines() if ".dmp" not in l)

    def drop_dumps_exclusion(d):
        w = diag_upload(d)["with"]
        w["path"] = "\n".join(l for l in w["path"].splitlines() if "dumps" not in l)

    def smoke_step(d):
        return next(x for x in d["jobs"]["build"]["steps"] if "smoke-windows.ps1" in str(x.get("run", "")))

    def dumps_step(d):
        return next(x for x in d["jobs"]["build"]["steps"] if "LocalDumps" in str(x.get("run", "")))

    def exe_step(d):
        return next(x for x in d["jobs"]["build"]["steps"] if "scan-exe-markers.ps1" in str(x.get("run", "")))

    def tauri_step(d):
        return next(x for x in d["jobs"]["build"]["steps"] if "tauri build" in str(x.get("run", "")))

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
        ("删掉 exe 标记扫描", lambda d, t: (d["jobs"]["build"].__setitem__("steps", [x for x in d["jobs"]["build"]["steps"] if "scan-exe-markers" not in str(x.get("run", ""))]), t), "缺少 exe 测试标记扫描"),
        ("exe 标记扫描改成 -Build Debug", lambda d, t: (exe_step(d).__setitem__("run", exe_step(d)["run"].replace("-Build Release", "-Build Debug")), t), "-Build Release"),
        ("exe 标记扫描 continue-on-error", lambda d, t: (exe_step(d).__setitem__("continue-on-error", True), t), "continue-on-error"),
        ("exe 标记扫描加 if", lambda d, t: (exe_step(d).__setitem__("if", "false"), t), "exe 测试标记扫描步骤不得有 if"),
        ("tauri build 启用 updater-e2e", lambda d, t: (tauri_step(d).__setitem__("run", tauri_step(d)["run"].replace("tauri build", "tauri build --features updater-e2e")), t), "updater-e2e"),
        ("smoke-diagnostics 保留 30 天", lambda d, t: (diag_upload(d)["with"].__setitem__("retention-days", 30), t), "retention-days"),
        ("smoke-diagnostics 保留期是表达式", lambda d, t: (diag_upload(d)["with"].__setitem__("retention-days", "${{ 3 }}"), t), "retention-days"),
        ("smoke-diagnostics 不排除 .dmp", lambda d, t: (drop_dmp_exclusion(d), t), "不得含内存转储"),
        ("smoke-diagnostics 不排除 dumps/", lambda d, t: (drop_dumps_exclusion(d), t), "不得含内存转储"),
        ("smoke-diagnostics 只排除 dumps/（不带 **）", lambda d, t: (diag_upload(d)["with"].__setitem__("path", diag_upload(d)["with"]["path"].replace("dumps/**", "dumps/")), t), "不得含内存转储"),
        ("smoke-crash-dumps 总是上传", lambda d, t: (dumps_upload(d).__setitem__("if", "${{ !cancelled() }}"), t), "冒烟失败时"),
        ("smoke-crash-dumps 任一步失败就上传", lambda d, t: (dumps_upload(d).__setitem__("if", "failure()"), t), "冒烟失败时"),
        ("smoke-crash-dumps 保留 30 天", lambda d, t: (dumps_upload(d)["with"].__setitem__("retention-days", 30), t), "smoke-crash-dumps 的 retention-days"),
        ("另一个 artifact 上传 dumps", lambda d, t: (dumps_upload(d)["with"].__setitem__("name", "dumps"), t), "只能由 smoke-crash-dumps"),
        ("冒烟步骤没有 id: smoke", lambda d, t: (smoke_step(d).pop("id"), t), "id 为 smoke"),
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


def check_file(path):
    doc, text = load(path)
    errs = (check_publish if is_publish(path) else check)(doc, text)
    for e in errs:
        print(f"{os.path.relpath(path)}: {e}")
    if errs:
        return 1
    print(f"check-release-workflow: 通过（{os.path.relpath(path)}）")
    return 0


def main(argv):
    args = argv[1:]
    if args[:1] == ["--self-test"]:
        if len(args) > 2:
            print("用法：check-release-workflow.py --self-test [文件]", file=sys.stderr)
            return 2
        if len(args) == 2:
            return (self_test_publish if is_publish(args[1]) else self_test)(args[1])
        rc = self_test(DEFAULT)
        return self_test_publish(DEFAULT_PUBLISH) or rc
    if len(args) > 1 or (args and args[0].startswith("-")):
        print("用法：check-release-workflow.py [文件] | --self-test [文件]", file=sys.stderr)
        return 2
    if args:
        return check_file(args[0])
    rc = check_file(DEFAULT)
    return check_file(DEFAULT_PUBLISH) or rc


if __name__ == "__main__":
    sys.exit(main(sys.argv))
