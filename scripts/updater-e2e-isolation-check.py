#!/usr/bin/env python3
"""updater-e2e 测试安装程序的隔离检查（docs/windows-updater.md「测试安装程序」）。

scripts/updater-e2e-exit-nonzero.nsi 编译出的测试安装程序只能在 updater-e2e.yml 的 e2e job 中现场生成，
只放进本地回放服务器的根目录，不得进入任何上传的 artifact，也不得被 release workflow 或打包配置引用。
本脚本只用标准库做文本检查：
  1. 引用测试安装程序（脚本名、标记、输出目录）的文件只能是白名单中的这几个；
  2. release-windows.yml 与 tauri*.conf.json 中不出现这些名字；
  3. updater-e2e.yml 中所有 upload-artifact 的 path 都不含测试安装程序的输出目录；
  4. 测试安装程序的标记字符串出现在 .nsi 中（cases 脚本据此在运行时扫描 release 形态产物）。
用法：python3 scripts/updater-e2e-isolation-check.py [--self-test]
"""
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
NSI = "scripts/updater-e2e-exit-nonzero.nsi"
MARKER = "CLASH_ASSIST_E2E_TEST_INSTALLER_EXIT_NONZERO"
OUT_DIR = "e2e-test-installer"
NEEDLES = ("updater-e2e-exit-nonzero", MARKER, OUT_DIR)
ALLOWED = {
    NSI,
    "scripts/updater-e2e-cases.ps1",
    "scripts/updater-e2e-isolation-check.py",
    ".github/workflows/updater-e2e.yml",
    "docs/windows-updater.md",
}
FORBIDDEN = (
    ".github/workflows/release-windows.yml",
    "src-tauri/tauri.conf.json",
    "src-tauri/tauri.windows.conf.json",
)


def upload_paths(workflow_text):
    """返回 upload-artifact 步骤 with.path 中的每一行（只认本仓库 workflow 的写法：`path:` 单行或 `path: |` 块）。"""
    lines = workflow_text.splitlines()
    out = []
    i = 0
    while i < len(lines):
        if re.search(r"uses:\s*actions/upload-artifact@", lines[i]):
            step_indent = len(lines[i]) - len(lines[i].lstrip(" -"))
            j = i + 1
            while j < len(lines):
                ln = lines[j]
                stripped = ln.strip()
                ind = len(ln) - len(ln.lstrip(" "))
                if stripped.startswith("- ") and ind < step_indent:
                    break
                m = re.match(r"\s*path:\s*(.*)$", ln)
                if m:
                    val = m.group(1).strip()
                    if val in ("|", "|-", ">", ">-"):
                        k = j + 1
                        while k < len(lines) and (not lines[k].strip() or len(lines[k]) - len(lines[k].lstrip(" ")) > ind):
                            if lines[k].strip():
                                out.append(lines[k].strip())
                            k += 1
                        j = k
                        continue
                    out.append(val)
                j += 1
            i = j
            continue
        i += 1
    return out


def check(files, read):
    errs = []
    for f in files:
        try:
            text = read(f)
        except (UnicodeDecodeError, OSError):
            continue
        hits = [n for n in NEEDLES if n in text]
        if hits and f not in ALLOWED:
            errs.append(f"{f} 引用了测试安装程序（{', '.join(hits)}），不在白名单中")
    for f in FORBIDDEN:
        if f in files:
            text = read(f)
            for n in NEEDLES:
                if n in text:
                    errs.append(f"{f} 不得引用 {n}")
    wf = ".github/workflows/updater-e2e.yml"
    if wf in files:
        for p in upload_paths(read(wf)):
            if OUT_DIR in p or p.endswith(".exe") or "root-*/" in p and not p.endswith("release.json"):
                errs.append(f"{wf} 的 upload-artifact 上传了可能含测试安装程序的路径：{p}")
    if NSI not in files or MARKER not in read(NSI):
        errs.append(f"{NSI} 缺失或不含标记 {MARKER}")
    return errs


def self_test():
    wf_ok = """
      - name: up
        uses: actions/upload-artifact@x # v
        with:
          name: r
          path: |
            ${{ runner.temp }}/e2e-results/results.json
            ${{ runner.temp }}/e2e-results/root-*/release.json
          retention-days: 7
"""
    wf_bad = wf_ok.replace("results.json\n", "results.json\n            ${{ runner.temp }}/e2e-test-installer/*\n", 1)
    wf_bad2 = """
      - uses: actions/upload-artifact@x
        with:
          path: ${{ runner.temp }}/e2e-results/root-*/
"""
    nsi = f'VIAddVersionKey "Comments" "{MARKER}"'
    cases = [
        ({NSI: nsi, ".github/workflows/updater-e2e.yml": wf_ok}, 0),
        ({NSI: nsi, ".github/workflows/updater-e2e.yml": wf_bad}, 1),
        ({NSI: nsi, ".github/workflows/updater-e2e.yml": wf_bad2}, 1),
        ({NSI: nsi, ".github/workflows/release-windows.yml": "run: makensis scripts/updater-e2e-exit-nonzero.nsi"}, 2),
        ({NSI: nsi, "src-tauri/tauri.conf.json": '{"resources": ["../scripts/updater-e2e-exit-nonzero.nsi"]}'}, 2),
        ({NSI: nsi, "scripts/other.sh": MARKER}, 1),
        ({NSI: "no marker"}, 1),
        ({}, 1),
    ]
    bad = 0
    for i, (fs, want) in enumerate(cases):
        got = check(set(fs), lambda f, fs=fs: fs[f])
        if len(got) != want:
            print(f"self-test #{i}：期望 {want} 条错误，实际 {got}")
            bad += 1
    paths = upload_paths(wf_ok)
    if paths != ["${{ runner.temp }}/e2e-results/results.json", "${{ runner.temp }}/e2e-results/root-*/release.json"]:
        print(f"self-test upload_paths：{paths}")
        bad += 1
    print("updater-e2e-isolation-check 自测：" + ("通过" if not bad else f"{bad} 项失败"))
    return 1 if bad else 0


def main():
    if "--self-test" in sys.argv[1:]:
        return self_test()
    files = set(subprocess.run(["git", "ls-files"], cwd=ROOT, check=True, capture_output=True, text=True).stdout.split("\n")) - {""}

    def read(f):
        with open(os.path.join(ROOT, f), encoding="utf-8") as fh:
            return fh.read()

    errs = check(files, read)
    for e in errs:
        print(f"::error::{e}")
    print("updater-e2e-isolation-check：" + ("通过" if not errs else f"{len(errs)} 处问题"))
    return 1 if errs else 0


if __name__ == "__main__":
    sys.exit(main())
