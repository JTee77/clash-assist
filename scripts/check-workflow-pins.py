#!/usr/bin/env python3
"""供应链检查（windows-plan r4 §2.8 / §3，S-8 ②④；PR #46 评审 SR-3 / SR-4）。

用真正的 YAML 解析器（PyYAML，ubuntu-24.04 runner 自带 python3-yaml）读取 .github 下
所有 *.yml / *.yaml，而不是按行正则匹配，因此引号键（"uses":）、复杂键（? uses）、
转义字符、锚点 / 别名都按 YAML 语义处理。

规则：
  1. 文档中任意位置的映射键 `uses`（step 级 action、job 级 reusable workflow、
     composite action 的 runs.steps）都必须是字符串，且为下列之一：
       - owner/repo[/path]@<40 位小写十六进制 commit SHA>
       - docker://image@sha256:<64 位十六进制 digest>
       - 本地 action / workflow：必须以 `./.github/` 开头，路径中不得含 `..` 段，
         用 realpath 解析符号链接后必须仍在仓库的 .github 目录内（PR #46 r2 MR-2）
     本地引用会被递归检查：action 目录下的 action.yml / action.yaml、或被调用的
     reusable workflow 文件，按同样规则再检查一遍（即使引用本身已违规，只要目标
     仍在仓库内也会继续检查，以便一次报全）。找不到 action.yml / action.yaml 视为违规。
  2. workflow（位于 workflows/ 目录下或含 `jobs` 键的文档）的触发器中
     禁止 pull_request_target 与 workflow_run。PyYAML 按 YAML 1.1 会把键 `on`
     解析成布尔 True，两种形式都检查。
  3. 同一映射中出现重复键视为违规（防止用重复键遮蔽检查结果）。

用法：python3 scripts/check-workflow-pins.py [--repo-root DIR] [路径 ...]
      路径可以是目录（递归查找 *.yml / *.yaml）或文件，默认 .github。
退出码：0 = 通过；1 = 发现违规；2 = 用法错误 / 文件无法解析 / 缺少 PyYAML。
"""

import argparse
import os
import re
import sys

try:
    import yaml
except ImportError:  # pragma: no cover - 环境问题直接失败，不静默放行
    print("check-workflow-pins: 需要 PyYAML（Debian/Ubuntu 包 python3-yaml）", file=sys.stderr)
    sys.exit(2)

SHA_RE = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(/[^@\s]+)?@[0-9a-f]{40}$")
DOCKER_RE = re.compile(r"^docker://[^@\s]+@sha256:[0-9a-f]{64}$")
LOCAL_PREFIX = "./.github/"
BANNED_TRIGGERS = ("pull_request_target", "workflow_run")


class DuplicateKeyError(Exception):
    pass


class StrictLoader(yaml.SafeLoader):
    """SafeLoader + 拒绝重复键。"""


def _construct_mapping(loader, node, deep=False):
    loader.flatten_mapping(node)
    seen = set()
    for key_node, _ in node.value:
        key = loader.construct_object(key_node, deep=deep)
        try:
            hashable = key in seen
        except TypeError:
            raise DuplicateKeyError(f"第 {key_node.start_mark.line + 1} 行：不可哈希的键")
        if hashable:
            raise DuplicateKeyError(f"第 {key_node.start_mark.line + 1} 行：重复键 {key!r}")
        seen.add(key)
    return yaml.SafeLoader.construct_mapping(loader, node, deep)


StrictLoader.add_constructor(yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG, _construct_mapping)


def collect_files(paths):
    files = []
    for p in paths:
        if os.path.isdir(p):
            for dirpath, _dirs, names in os.walk(p):
                for n in names:
                    if n.endswith((".yml", ".yaml")):
                        files.append(os.path.join(dirpath, n))
        elif os.path.isfile(p):
            files.append(p)
        else:
            print(f"check-workflow-pins: 路径不存在：{p}", file=sys.stderr)
            sys.exit(2)
    return sorted(files)


def _inside(path, root):
    return path == root or path.startswith(root + os.sep)


def check_local(ref, repo_root):
    """返回 (错误列表, 需要递归检查的文件 realpath 或 None)。"""
    errs = []
    rel = ref[2:]
    segments = rel.replace("\\", "/").split("/")
    if not rel or ".." in segments:
        return ["本地路径不得为空或包含 `..` 段"], None
    if not ref.startswith(LOCAL_PREFIX):
        errs.append(f"本地 action / workflow 只允许位于 {LOCAL_PREFIX} 下")
    root = os.path.realpath(repo_root)
    gh_root = os.path.join(root, ".github")
    target = os.path.realpath(os.path.join(root, rel))
    if not _inside(target, root):
        return errs + ["本地路径解析后不在仓库内"], None
    if not _inside(target, gh_root):
        errs.append("本地路径（解析符号链接后）不在仓库的 .github 目录内")
    if os.path.isfile(target):
        return errs, target
    for name in ("action.yml", "action.yaml"):
        cand = os.path.join(target, name)
        if os.path.isfile(cand):
            # action.yml 本身也可能是指向别处的符号链接
            real = os.path.realpath(cand)
            if not _inside(real, root):
                return errs + [f"{name} 解析后不在仓库内"], None
            if _inside(target, gh_root) and not _inside(real, gh_root):
                errs.append(f"{name}（解析符号链接后）不在仓库的 .github 目录内")
            return errs, real
    return errs + ["找不到本地 action 的 action.yml / action.yaml，无法检查"], None


def check_uses(value, repo_root):
    """返回 (错误列表, 需要递归检查的本地文件或 None)。"""
    if not isinstance(value, str):
        return [f"uses 必须是字符串，实际为 {type(value).__name__}"], None
    if value.startswith("./"):
        return check_local(value, repo_root)
    if value.startswith("docker://"):
        return ([] if DOCKER_RE.match(value) else ["docker:// 必须按 @sha256:<64 位十六进制> 固定"]), None
    return ([] if SHA_RE.match(value) else ["未按 40 位 commit SHA 固定"]), None


def walk_uses(node, path, out):
    if isinstance(node, dict):
        for k, v in node.items():
            p = f"{path}.{k}" if path else str(k)
            if k == "uses":
                out.append((p, v))
            walk_uses(v, p, out)
    elif isinstance(node, list):
        for i, v in enumerate(node):
            walk_uses(v, f"{path}[{i}]", out)


def trigger_names(on):
    if isinstance(on, str):
        return [on]
    if isinstance(on, list):
        return [x for x in on if isinstance(x, str)]
    if isinstance(on, dict):
        return [k for k in on.keys() if isinstance(k, str)]
    return []


def check_file(f, repo_root, state):
    """检查单个 YAML 文件；本地引用指向的文件加入队列。返回 False 表示无法解析。"""
    try:
        with open(f, encoding="utf-8") as fh:
            docs = list(yaml.load_all(fh, Loader=StrictLoader))
    except DuplicateKeyError as e:
        state["problems"].append(f"{f}: {e}")
        return True
    except (yaml.YAMLError, UnicodeDecodeError, OSError) as e:
        print(f"check-workflow-pins: 无法解析 {f}：{e}", file=sys.stderr)
        return False
    in_workflows = os.sep + "workflows" + os.sep in os.sep + os.path.normpath(f)
    for doc in docs:
        found = []
        walk_uses(doc, "", found)
        state["uses"] += len(found)
        for p, v in found:
            errs, follow = check_uses(v, repo_root)
            for err in errs:
                state["problems"].append(f"{f}: {p}: {err}：{v!r}")
            if follow:
                state["queue"].append(follow)
        if isinstance(doc, dict) and (in_workflows or "jobs" in doc):
            for key in ("on", True):
                if key in doc:
                    for t in trigger_names(doc[key]):
                        if t in BANNED_TRIGGERS:
                            state["problems"].append(f"{f}: on: 禁止的触发器 {t}")
    return True


def main(argv):
    ap = argparse.ArgumentParser(add_help=True)
    ap.add_argument("--repo-root", default=".")
    ap.add_argument("paths", nargs="*", default=[".github"])
    args = ap.parse_args(argv)

    files = collect_files(args.paths)
    if not files:
        print(f"check-workflow-pins: {' '.join(args.paths)} 下没有 YAML 文件", file=sys.stderr)
        return 2

    state = {"problems": [], "uses": 0, "queue": list(files)}
    seen = set()
    checked = 0
    while state["queue"]:
        f = state["queue"].pop(0)
        real = os.path.realpath(f)
        if real in seen:
            continue
        seen.add(real)
        checked += 1
        if not check_file(f, args.repo_root, state):
            return 2

    problems = state["problems"]
    if problems:
        for p in problems:
            print(p)
        print("check-workflow-pins: 失败（见上方列表）")
        return 1
    print(f"check-workflow-pins: 通过（{checked} 个文件，{state['uses']} 处 uses）")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
