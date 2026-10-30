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
     所有规则都按「整个值」匹配（re.fullmatch），值末尾的换行 / 空白不能混过（r2 SR2-3 n16）；
     远程引用的 owner/repo/path 各段不得为空、`.` 或 `..`（r2 SR2-3 n17）。
  2. 容器镜像必须按 digest 固定（r2 SR2-2）：
       - workflow 的 jobs.*.container（字符串或 .image）、jobs.*.services.*.image：
         [docker://]image@sha256:<64 位十六进制>
       - docker action（action.yml 中 runs.using: docker）的 runs.image：
         docker://image@sha256:<64 位十六进制>，或指向仓库内 Dockerfile 的相对路径
         （不得含 `..` 段、不得为绝对路径，解析后必须仍在仓库内）
  3. workflow（位于 workflows/ 目录下或含 `jobs` 键的文档）的触发器中
     禁止 pull_request_target 与 workflow_run。PyYAML 按 YAML 1.1 会把键 `on`
     解析成布尔 True，两种形式都检查。
  4. 同一映射中出现重复键视为违规（防止用重复键遮蔽检查结果）。

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

# 只与 re.fullmatch 搭配使用（不用 ^/$：Python 的 $ 会匹配末尾的 \n）
SHA_RE = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(/[^@\s]+)?@[0-9a-f]{40}")
DOCKER_RE = re.compile(r"docker://[^@\s]+@sha256:[0-9a-f]{64}")
IMAGE_RE = re.compile(r"(docker://)?[^@\s]+@sha256:[0-9a-f]{64}")
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


def _scan_to_next_token(self):
    """与 PyYAML 的 Scanner.scan_to_next_token 相同，但允许注释或行尾之前出现 Tab。

    YAML 1.2 允许用 Tab 分隔值与行内注释（`uses: x@sha<Tab># v4`），PyYAML 会直接报
    「found character '\\t' that cannot start any token」（r2 c22 误报）。这里只放行
    「一串空格 / Tab 之后紧跟 # 或换行」的情况；Tab 出现在其它位置（如缩进）仍按原样报错。
    """
    if self.index == 0 and self.peek() == "\uFEFF":
        self.forward()
    found = False
    while not found:
        while self.peek() == " ":
            self.forward()
        if self.peek() == "\t":
            n = 0
            while self.peek(n) in " \t":
                n += 1
            if self.peek(n) in "#\0\r\n\x85\u2028\u2029":
                self.forward(n)
        if self.peek() == "#":
            while self.peek() not in "\0\r\n\x85\u2028\u2029":
                self.forward()
        if self.scan_line_break():
            if not self.flow_level:
                self.allow_simple_key = True
        else:
            found = True


StrictLoader.scan_to_next_token = _scan_to_next_token
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
    if re.search(r"\s", ref):
        return ["本地路径不得含空白字符（含末尾换行）"], None
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
        return ([] if DOCKER_RE.fullmatch(value) else ["docker:// 必须按 @sha256:<64 位十六进制> 固定"]), None
    if not SHA_RE.fullmatch(value):
        return ["未按 40 位 commit SHA 固定"], None
    if any(seg in ("", ".", "..") for seg in value.split("@", 1)[0].split("/")):
        return ["远程引用的路径段不得为空、`.` 或 `..`"], None
    return [], None


def check_image(value):
    if not isinstance(value, str):
        return f"镜像必须是字符串，实际为 {type(value).__name__}"
    return None if IMAGE_RE.fullmatch(value) else "容器镜像必须按 @sha256:<64 位十六进制> 固定"


def check_job_images(doc):
    """workflow 中 jobs.*.container / jobs.*.services.*.image。返回 [(路径, 值, 错误)]。"""
    out = []
    jobs = doc.get("jobs")
    if not isinstance(jobs, dict):
        return out
    for name, job in jobs.items():
        if not isinstance(job, dict):
            continue
        if "container" in job:
            c = job["container"]
            p = f"jobs.{name}.container"
            if isinstance(c, dict):
                c, p = c.get("image"), p + ".image"
            err = check_image(c)
            if err:
                out.append((p, c, err))
        services = job.get("services")
        if isinstance(services, dict):
            for sname, svc in services.items():
                img = svc.get("image") if isinstance(svc, dict) else svc
                err = check_image(img)
                if err:
                    out.append((f"jobs.{name}.services.{sname}.image", img, err))
    return out


def check_docker_action(doc, f, repo_root):
    """action.yml 中 runs.using: docker 的 runs.image。返回错误或 None。"""
    runs = doc.get("runs")
    if not isinstance(runs, dict) or runs.get("using") != "docker":
        return None
    img = runs.get("image")
    if not isinstance(img, str):
        return f"runs.image 必须是字符串，实际为 {type(img).__name__}"
    if img.startswith("docker://"):
        return None if DOCKER_RE.fullmatch(img) else "runs.image 的 docker:// 镜像必须按 @sha256:<64 位十六进制> 固定"
    # 否则是相对 action 目录的 Dockerfile 路径
    if re.search(r"\s", img) or img.startswith(("/", "\\")) or ".." in img.replace("\\", "/").split("/"):
        return "runs.image 的 Dockerfile 路径不得为绝对路径、不得含 `..` 段或空白字符"
    root = os.path.realpath(repo_root)
    target = os.path.realpath(os.path.join(os.path.dirname(os.path.realpath(f)), img))
    if not _inside(target, root):
        return "runs.image 的 Dockerfile 解析后不在仓库内"
    return None


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
            text = fh.read()
        docs = list(yaml.load_all(text, Loader=StrictLoader))
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
        if isinstance(doc, dict):
            for p, v, err in check_job_images(doc):
                state["problems"].append(f"{f}: {p}: {err}：{v!r}")
            err = check_docker_action(doc, f, repo_root)
            if err:
                state["problems"].append(f"{f}: {err}：{doc['runs'].get('image')!r}")
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
