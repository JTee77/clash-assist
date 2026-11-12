#!/usr/bin/env python3
"""检查 Windows 构建产物里的图标资源是否与配置引用的 .ico 一致。

对照对象是「同一提交里的源文件」，因此在更换 icon.ico 或补上 installerIcon 前后都能用：

1. 应用 exe（clash-assist.exe）的 RT_GROUP_ICON / RT_ICON 必须与 bundle.icon 里的
   .ico（目前是 icons/icon.ico）尺寸集合（宽×高×色深）和每帧图像字节完全一致。
2. 若配置了 bundle.windows.nsis.installerIcon（会合并 tauri.conf.json 与
   tauri.windows.conf.json）：NSIS 安装器 exe 的图标资源必须与该 .ico 同样对照，
   配置了却缺图标或对不上即失败。
3. 若未配置 installerIcon：不把安装器图标记为通过。只报告安装器里实际嵌入的
   默认图标（尺寸、格式、是否碰巧与 icon.ico 相同），供人工核对默认行为。

用法：
  python3 scripts/check-windows-icons.py --self-test
  python3 scripts/check-windows-icons.py --exe <app.exe> [--installer <setup.exe>]
退出码：0 = 通过（未配置 installerIcon 时安装器项为报告、不记通过）；1 = 失败；2 = 用法错误。
只用 Python 标准库。
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import io
import json
import struct
import sys
import tempfile
import zlib
from collections import Counter, defaultdict
from pathlib import Path

RT_ICON = 3
RT_GROUP_ICON = 14
# 资源树正常只有三层目录：类型 / 名称或 id / 语言，叶子是 IMAGE_RESOURCE_DATA_ENTRY。
# 更深的子目录或指回已访问目录（成环）都视为畸形 PE，直接失败，避免递归失控。
MAX_RESOURCE_DIR_DEPTH = 3


def _ensure_utf8_stdio() -> None:
    """Windows CI 的控制台默认是 cp1252，直接 print 中文会 UnicodeEncodeError。"""
    for stream in (sys.stdout, sys.stderr):
        reconfigure = getattr(stream, "reconfigure", None)
        if callable(reconfigure):
            try:
                reconfigure(encoding="utf-8", errors="replace")
            except (OSError, ValueError, AttributeError):
                pass


ROOT = Path(__file__).resolve().parent.parent
DEFAULT_CONF = ROOT / "src-tauri" / "tauri.conf.json"
DEFAULT_WIN_CONF = ROOT / "src-tauri" / "tauri.windows.conf.json"


# ---------------------------------------------------------------------------
# ICO / PE 解析
# ---------------------------------------------------------------------------

def sha16(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()[:16]


def kind_of(img: bytes) -> str:
    if img.startswith(b"\x89PNG\r\n\x1a\n"):
        return "PNG"
    if len(img) >= 4 and struct.unpack_from("<I", img, 0)[0] in (12, 40, 108, 124):
        return "DIB"
    return "other"


def infer_bpp(img: bytes, directory_bpp: int) -> int:
    if directory_bpp:
        return directory_bpp
    if img.startswith(b"\x89PNG\r\n\x1a\n") and len(img) >= 25:
        # IHDR: bit depth at +24, color type at +25
        bit_depth = img[24]
        color_type = img[25]
        if color_type == 6:
            return bit_depth * 4
        if color_type == 2:
            return bit_depth * 3
        if color_type in (0, 3):
            return bit_depth
    if len(img) >= 16 and struct.unpack_from("<I", img, 0)[0] >= 16:
        # BITMAPINFOHEADER: biBitCount at +14
        return struct.unpack_from("<H", img, 14)[0]
    return 0


def parse_ico(data: bytes, name: str = "ico") -> list[dict]:
    """解析 ICO；截断等畸形输入一律以 ValueError 报出（不让 struct.error 冒出去）。"""
    try:
        return _parse_ico(data, name)
    except struct.error as e:
        raise ValueError(f"{name}：ICO 数据截断或畸形（{e}）") from e


def _parse_ico(data: bytes, name: str) -> list[dict]:
    if len(data) < 6:
        raise ValueError(f"{name}：文件太短，不是 ICO")
    reserved, typ, count = struct.unpack_from("<HHH", data, 0)
    if reserved != 0 or typ != 1 or count < 1:
        raise ValueError(f"{name}：不是有效的 ICO（reserved={reserved} type={typ} count={count}）")
    entries = []
    off = 6
    for i in range(count):
        if off + 16 > len(data):
            raise ValueError(f"{name}：目录项 {i} 超出文件")
        w, h, colors, _res, planes, bpp, size, image_off = struct.unpack_from("<BBBBHHII", data, off)
        if image_off + size > len(data):
            raise ValueError(f"{name}：目录项 {i} 的图像超出文件")
        img = data[image_off : image_off + size]
        ww, hh = (w or 256), (h or 256)
        entries.append(
            {
                "w": ww,
                "h": hh,
                "bpp": infer_bpp(img, bpp),
                "planes": planes,
                "colors": colors,
                # 目录项原样（宽, 高, 颜色数, 保留, planes, 色深, 字节数），供安装器逐字段对照
                "entry": (w, h, colors, _res, planes, bpp, size),
                "data": img,
                "kind": kind_of(img),
            }
        )
        off += 16
    return entries


def _rva_to_off(sections, rva: int) -> int:
    for va, _vs, raw_off, raw_size, _name in sections:
        if va <= rva < va + max(_vs, raw_size):
            return raw_off + (rva - va)
    raise ValueError(f"RVA {rva:#x} 不在任何节里")


def parse_pe_icon_groups(data: bytes, name: str = "exe", *, nsis_zero_padding: bool = False) -> list[list[dict]]:
    """返回该 PE 里每一个 RT_GROUP_ICON 的帧列表（已把 RT_ICON 图像填上）。

    默认要求 RT_ICON 的长度与组目录声明的字节数完全相等。

    nsis_zero_padding=True 只给 NSIS 安装器用：makensis 让安装器与卸载器图标共用 RT_ICON 槽，
    槽长取两者中较大的一帧，先 memset 0 再拷入安装器那一帧（NSIS v3.11 Source/icon.cpp
    get_icon_order 第 248–298 行、set_main_icon 第 349–367 行）。因此允许 RT_ICON 比声明的长，
    但多出的尾部必须全是 0x00；比声明的短、或尾部有任何非零字节，照样失败。返回的帧数据只取声明的长度。

    截断或畸形的 PE（struct.error、资源树过深或成环）一律以 ValueError 报出，
    由调用方记为检查失败，而不是抛出 traceback。"""
    try:
        return _parse_pe_icon_groups(data, name, nsis_zero_padding)
    except struct.error as e:
        raise ValueError(f"{name}：PE 数据截断或畸形（{e}）") from e


def _parse_pe_icon_groups(data: bytes, name: str, nsis_zero_padding: bool) -> list[list[dict]]:
    if len(data) < 64 or data[:2] != b"MZ":
        raise ValueError(f"{name}：不是 MZ/PE")
    e_lfanew = struct.unpack_from("<I", data, 0x3C)[0]
    if e_lfanew + 4 > len(data) or data[e_lfanew : e_lfanew + 4] != b"PE\0\0":
        raise ValueError(f"{name}：不是 PE")
    coff = e_lfanew + 4
    _machine, nsec, _ts, _ps, _pn, optsz, _chars = struct.unpack_from("<HHIIIHH", data, coff)
    opt = coff + 20
    if opt + 2 > len(data):
        raise ValueError(f"{name}：可选头缺失")
    magic = struct.unpack_from("<H", data, opt)[0]
    pe32plus = magic == 0x20B
    if magic not in (0x10B, 0x20B):
        raise ValueError(f"{name}：无法识别的可选头 magic {magic:#x}")
    if pe32plus:
        num_dd = struct.unpack_from("<I", data, opt + 108)[0]
        dd_off = opt + 112
    else:
        num_dd = struct.unpack_from("<I", data, opt + 92)[0]
        dd_off = opt + 96
    if num_dd < 3:
        raise ValueError(f"{name}：没有资源数据目录")
    res_rva, res_size = struct.unpack_from("<II", data, dd_off + 16)
    if res_rva == 0 or res_size == 0:
        raise ValueError(f"{name}：没有资源节")
    sec_off = opt + optsz
    sections = []
    for i in range(nsec):
        s = sec_off + i * 40
        if s + 40 > len(data):
            raise ValueError(f"{name}：节表截断")
        sec_name = data[s : s + 8].rstrip(b"\0")
        vs, va, raw_size, raw_off = struct.unpack_from("<IIII", data, s + 8)
        sections.append((va, vs, raw_off, raw_size, sec_name))

    def read(rva: int, n: int) -> bytes:
        off = _rva_to_off(sections, rva)
        if off < 0 or off + n > len(data):
            raise ValueError(f"{name}：资源读取越界 RVA {rva:#x}+{n}")
        return data[off : off + n]

    visited: set[int] = set()

    def parse_dir(rva: int, path: tuple = ()):
        if len(path) >= MAX_RESOURCE_DIR_DEPTH:
            raise ValueError(
                f"{name}：资源目录嵌套超过 {MAX_RESOURCE_DIR_DEPTH} 层（路径 {path!r}），视为畸形 PE"
            )
        if rva in visited:
            raise ValueError(f"{name}：资源目录成环（RVA {rva:#x} 被重复引用），视为畸形 PE")
        visited.add(rva)
        buf = read(rva, 16)
        named, idcnt = struct.unpack_from("<HH", buf, 12)
        total = named + idcnt
        raw = read(rva + 16, total * 8)
        out = []
        for i in range(total):
            name_or_id, data_or_sub = struct.unpack_from("<II", raw, i * 8)
            ident = name_or_id & 0x7FFFFFFF
            if name_or_id & 0x80000000:
                ident = ("name", ident)
            ptr = data_or_sub & 0x7FFFFFFF
            if data_or_sub & 0x80000000:
                out.extend(parse_dir(res_rva + ptr, path + (ident,)))
            else:
                de = read(res_rva + ptr, 16)
                d_rva, d_size, d_codepage, _ = struct.unpack_from("<IIII", de)
                blob = read(d_rva, d_size)
                out.append({"path": path + (ident,), "data": blob})
        return out

    entries = parse_dir(res_rva)
    icons: dict[object, bytes] = {}
    groups_raw = []
    for e in entries:
        typ = e["path"][0] if e["path"] else None
        rid = e["path"][1] if len(e["path"]) > 1 else None
        if typ == RT_ICON:
            icons[rid] = e["data"]
        elif typ == RT_GROUP_ICON:
            groups_raw.append((rid, e["data"]))

    groups = []
    for gid, raw in groups_raw:
        if len(raw) < 6:
            raise ValueError(f"{name}：RT_GROUP_ICON id={gid} 太短")
        reserved, typ, count = struct.unpack_from("<HHH", raw, 0)
        if reserved != 0 or typ != 1 or count < 1:
            raise ValueError(
                f"{name}：RT_GROUP_ICON id={gid} 不是图标组（reserved={reserved} type={typ} count={count}）"
            )
        frames = []
        off = 6
        for i in range(count):
            if off + 14 > len(raw):
                raise ValueError(f"{name}：RT_GROUP_ICON id={gid} 目录项 {i} 截断")
            w, h, colors, _res, planes, bpp, size, nid = struct.unpack_from("<BBBBHHIH", raw, off)
            img = icons.get(nid)
            if img is None:
                raise ValueError(f"{name}：RT_GROUP_ICON id={gid} 引用了缺失的 RT_ICON id={nid}")
            pad = 0
            if len(img) != size:
                if not nsis_zero_padding or len(img) < size:
                    raise ValueError(
                        f"{name}：RT_ICON id={nid} 大小 {len(img)} 与组目录声明的 {size} 不一致"
                    )
                tail = img[size:]
                if tail.count(0) != len(tail):
                    first = next(i for i, b in enumerate(tail) if b)
                    raise ValueError(
                        f"{name}：RT_ICON id={nid} 大小 {len(img)} 超出组目录声明的 {size}，"
                        f"且多出的 {len(tail)} 字节不全是 0（第 {size + first} 字节为 {tail[first]:#04x}；"
                        "NSIS 共用图标槽只会补 0）"
                    )
                pad = len(tail)
                img = img[:size]
            ww, hh = (w or 256), (h or 256)
            frames.append(
                {
                    "w": ww,
                    "h": hh,
                    "bpp": infer_bpp(img, bpp),
                    "planes": planes,
                    "colors": colors,
                    "id": nid,
                    "group_id": gid,
                    "entry": (w, h, colors, _res, planes, bpp, size),
                    "pad": pad,
                    "data": img,
                    "kind": kind_of(img),
                }
            )
            off += 14
        groups.append(frames)
    return groups


def format_frames(frames: list[dict]) -> str:
    parts = []
    for f in frames:
        parts.append(f"{f['w']}x{f['h']}@{f['bpp']}({f['kind']},{sha16(f['data'])})")
    return ", ".join(parts)


def size_set(frames: list[dict]) -> list[tuple[int, int, int]]:
    return sorted((f["w"], f["h"], f["bpp"]) for f in frames)


def payloads_by_key(frames: list[dict]) -> dict[tuple[int, int, int], Counter]:
    out: dict[tuple[int, int, int], Counter] = defaultdict(Counter)
    for f in frames:
        out[(f["w"], f["h"], f["bpp"])][f["data"]] += 1
    return out


def compare_frames(got: list[dict], expected: list[dict], got_name: str, exp_name: str) -> list[str]:
    """尺寸集合与每帧图像字节都必须一致（不比较目录里的 planes/colors 字段，
    因为 rc.exe / 链接器常把 ICO 里的 planes=0 写成 1）。"""
    errs = []
    gs, es = size_set(got), size_set(expected)
    if gs != es:
        errs.append(
            f"{got_name} 的尺寸集合是 {gs}，与 {exp_name} 的 {es} 不一致"
        )
        return errs
    got_map, exp_map = payloads_by_key(got), payloads_by_key(expected)
    for key in es:
        if got_map[key] != exp_map[key]:
            errs.append(
                f"{got_name} 的 {key[0]}x{key[1]}@{key[2]} 图像字节与 {exp_name} 不一致"
                f"（产物 {sha16(next(iter(got_map[key])))} / 源 {sha16(next(iter(exp_map[key])))}）"
            )
    return errs


def compare_entries(got: list[dict], expected: list[dict], got_name: str, exp_name: str) -> list[str]:
    """逐字段对照组目录项（宽、高、颜色数、保留、planes、色深、字节数）与 .ico 目录项，连同图像字节。

    只用于 NSIS 安装器：makensis 把 .ico 的目录项原样 memcpy 进 RT_GROUP_ICON
    （NSIS v3.11 Source/icon.cpp generate_icon_group 第 323 行），所以必须逐字段相等。
    应用 exe 由 rc 编译，planes 等字段可能被改写，不走这条。"""
    g = Counter((f["entry"], f["data"]) for f in got)
    e = Counter((f["entry"], f["data"]) for f in expected)
    if g == e:
        return []
    errs = []
    for (entry, data), n in sorted((g - e).items(), key=lambda kv: kv[0][0]):
        errs.append(f"{got_name} 有 {exp_name} 里没有的目录项 {entry}（{kind_of(data)},{sha16(data)}）×{n}")
    for (entry, data), n in sorted((e - g).items(), key=lambda kv: kv[0][0]):
        errs.append(f"{got_name} 缺少 {exp_name} 的目录项 {entry}（{kind_of(data)},{sha16(data)}）×{n}")
    return errs


# ---------------------------------------------------------------------------
# 配置
# ---------------------------------------------------------------------------

def deep_merge(base, overlay):
    if isinstance(base, dict) and isinstance(overlay, dict):
        out = dict(base)
        for k, v in overlay.items():
            out[k] = deep_merge(out[k], v) if k in out else v
        return out
    return overlay


def load_json(path: Path) -> dict:
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as e:
        raise ValueError(f"无法读取 {path}：{e}") from e
    try:
        data = json.loads(text)
    except json.JSONDecodeError as e:
        raise ValueError(f"{path} 不是合法 JSON：{e}") from e
    if not isinstance(data, dict):
        raise ValueError(f"{path} 根节点不是对象")
    return data


def load_merged_config(conf_path: Path, win_conf_path: Path | None) -> tuple[dict, Path]:
    conf = load_json(conf_path)
    if win_conf_path is not None and win_conf_path.exists():
        conf = deep_merge(conf, load_json(win_conf_path))
    return conf, conf_path.parent


def bundle_ico_path(conf: dict, conf_dir: Path) -> Path:
    icons = (conf.get("bundle") or {}).get("icon") or []
    if not isinstance(icons, list) or not icons:
        raise ValueError("bundle.icon 为空，无法确定应用图标")
    icos = [p for p in icons if isinstance(p, str) and p.lower().endswith(".ico")]
    if not icos:
        raise ValueError(f"bundle.icon 里没有 .ico：{icons!r}")
    path = (conf_dir / icos[0]).resolve()
    if not path.is_file():
        raise ValueError(f"bundle.icon 引用的 {icos[0]} 不存在：{path}")
    return path


def installer_icon_path(conf: dict, conf_dir: Path) -> Path | None:
    nsis = ((conf.get("bundle") or {}).get("windows") or {}).get("nsis") or {}
    if not isinstance(nsis, dict):
        return None
    raw = nsis.get("installerIcon")
    if raw is None or (isinstance(raw, str) and not raw.strip()):
        return None
    if not isinstance(raw, str):
        raise ValueError(f"bundle.windows.nsis.installerIcon 必须是字符串，实际：{raw!r}")
    path = (conf_dir / raw).resolve()
    if not path.is_file():
        raise ValueError(f"installerIcon 引用的 {raw} 不存在：{path}")
    return path


# ---------------------------------------------------------------------------
# 检查
# ---------------------------------------------------------------------------

def check_pe_against_ico(pe_path: Path, ico_path: Path, label: str, *, nsis: bool = False) -> list[str]:
    """nsis=True（安装器）：允许 NSIS 共用图标槽留下的全零尾部，同时逐字段对照目录项。"""
    try:
        pe = pe_path.read_bytes()
    except OSError as e:
        return [f"无法读取 {label} {pe_path}：{e}"]
    try:
        ico = parse_ico(ico_path.read_bytes(), str(ico_path))
    except (OSError, ValueError) as e:
        return [str(e)]
    try:
        groups = parse_pe_icon_groups(pe, str(pe_path), nsis_zero_padding=nsis)
    except ValueError as e:
        return [str(e)]
    if not groups:
        return [f"{label} {pe_path} 没有 RT_GROUP_ICON"]
    # 应用 / 安装器通常各只有一个图标组；若有多个，任意一组完全匹配即可。
    errs_per_group = []
    for g in groups:
        gname = f"{label} 图标组 id={g[0]['group_id']}"
        errs = compare_frames(g, ico, gname, str(ico_path))
        if nsis and not errs:
            errs = compare_entries(g, ico, gname, str(ico_path))
        errs_per_group.append(errs)
    if any(not e for e in errs_per_group):
        matched = next(g for g, e in zip(groups, errs_per_group) if not e)
        print(
            f"check-windows-icons：{label} 通过，与 {ico_path.relative_to(ROOT) if ROOT in ico_path.parents else ico_path} "
            f"一致（{len(matched)} 帧：{format_frames(matched)}）"
        )
        padded = [f"id={f['id']} {f['w']}x{f['h']} +{f['pad']}" for f in matched if f["pad"]]
        if padded:
            print(
                "check-windows-icons：  目录项逐字段一致；以下 RT_ICON 槽比声明的长，尾部已核对全为 0"
                f"（NSIS 安装器/卸载器图标共用槽）：{', '.join(padded)}"
            )
        return []
    detail = [f"{label} {pe_path} 的 {len(groups)} 个图标组都与 {ico_path} 不一致："]
    for i, (g, errs) in enumerate(zip(groups, errs_per_group)):
        detail.append(f"  组 {i}（id={g[0]['group_id']}，{format_frames(g)}）：")
        detail.extend(f"    - {e}" for e in errs)
    return detail


def report_default_installer(pe_path: Path, ico_path: Path) -> int:
    """未配置 installerIcon：报告默认图标，不记为通过，也不因此失败。"""
    try:
        pe = pe_path.read_bytes()
        groups = parse_pe_icon_groups(pe, str(pe_path), nsis_zero_padding=True)
    except (OSError, ValueError) as e:
        print(f"check-windows-icons：无法读取安装器以报告默认图标：{e}", file=sys.stderr)
        return 1
    try:
        ico_frames = parse_ico(ico_path.read_bytes(), str(ico_path)) if ico_path.is_file() else []
    except (OSError, ValueError) as e:
        print(f"check-windows-icons：无法读取 {ico_path}：{e}", file=sys.stderr)
        return 1
    print("check-windows-icons：bundle.windows.nsis.installerIcon 未配置。")
    print("  此项不记为通过；配置该项后，安装器必须与所引用的 .ico 严格对照。")
    if not groups:
        print(f"  安装器 {pe_path} 里没有 RT_GROUP_ICON。")
        return 0
    for i, g in enumerate(groups):
        kinds = sorted({f["kind"] for f in g})
        print(f"  默认图标组 {i}（id={g[0]['group_id']}，{len(g)} 帧，格式 {','.join(kinds)}）：{format_frames(g)}")
        if ico_frames:
            errs = compare_frames(g, ico_frames, "安装器默认图标", str(ico_path))
            if errs:
                print(f"  与 {ico_path.name} 对照：不一致（当前默认如此，符合未配置 installerIcon 的预期）。")
                for e in errs:
                    print(f"    - {e}")
            else:
                print(f"  与 {ico_path.name} 对照：图像碰巧一致，但因未配置 installerIcon，仍不记为通过。")
    return 0


def run_check(exe: Path | None, installer: Path | None, conf_path: Path, win_conf_path: Path | None) -> int:
    try:
        conf, conf_dir = load_merged_config(conf_path, win_conf_path)
        app_ico = bundle_ico_path(conf, conf_dir)
        inst_ico = installer_icon_path(conf, conf_dir)
        app_ico_frames = parse_ico(app_ico.read_bytes(), str(app_ico))
    except (OSError, ValueError) as e:
        print(f"check-windows-icons：{e}", file=sys.stderr)
        return 1

    rel_app = app_ico.relative_to(ROOT) if ROOT in app_ico.parents else app_ico
    print(f"check-windows-icons：应用图标源 {rel_app}（{len(app_ico_frames)} 帧）")
    if inst_ico is not None:
        rel_inst = inst_ico.relative_to(ROOT) if ROOT in inst_ico.parents else inst_ico
        print(f"check-windows-icons：installerIcon = {rel_inst}")
    else:
        print("check-windows-icons：installerIcon 未配置")

    rc = 0
    if exe is not None:
        errs = check_pe_against_ico(exe, app_ico, "应用 exe")
        if errs:
            rc = 1
            for e in errs:
                print(f"check-windows-icons：{e}", file=sys.stderr)
    else:
        print("check-windows-icons：未提供 --exe，跳过应用 exe 检查")

    if installer is not None:
        if inst_ico is not None:
            errs = check_pe_against_ico(installer, inst_ico, "NSIS 安装器", nsis=True)
            if errs:
                rc = 1
                print(
                    "check-windows-icons：已配置 installerIcon，但安装器图标与该文件不一致"
                    "（配置了却缺资源或对不上即失败）。",
                    file=sys.stderr,
                )
                for e in errs:
                    print(f"check-windows-icons：{e}", file=sys.stderr)
        else:
            report_rc = report_default_installer(installer, app_ico)
            if report_rc != 0:
                rc = report_rc
    elif inst_ico is not None:
        print(
            "check-windows-icons：已配置 installerIcon，但本次没有传入 --installer，"
            "无法验证安装器产物（此项由 release-windows.yml 在打包后检查）。"
        )
    return rc


# ---------------------------------------------------------------------------
# 自测：生成最小 PNG/ICO 与带资源节的 PE
# ---------------------------------------------------------------------------

def _png_chunk(tag: bytes, data: bytes) -> bytes:
    return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)


def make_png(w: int, h: int, rgba: bytes) -> bytes:
    if len(rgba) != 4:
        raise ValueError("rgba 必须是 4 字节")
    raw = b"".join(b"\x00" + (rgba * w) for _ in range(h))
    ihdr = struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + _png_chunk(b"IHDR", ihdr) + _png_chunk(b"IDAT", zlib.compress(raw, 9)) + _png_chunk(b"IEND", b"")


def make_ico(frames: list[tuple[int, int, bytes]]) -> bytes:
    count = len(frames)
    header = struct.pack("<HHH", 0, 1, count)
    offset = 6 + 16 * count
    entries = b""
    images = b""
    for w, h, img in frames:
        entries += struct.pack(
            "<BBBBHHII",
            0 if w >= 256 else w,
            0 if h >= 256 else h,
            0,
            0,
            0,
            32,
            len(img),
            offset,
        )
        images += img
        offset += len(img)
    return header + entries + images


def make_pe_with_icons(
    frames: list[tuple[int, int, bytes]],
    *,
    group_id: int = 1,
    planes: int = 1,
    slots: list[bytes] | None = None,
) -> bytes:
    """构造一个最小 PE32，内含给定 PNG/DIB 帧的 RT_ICON + 一个 RT_GROUP_ICON。

    组目录项按 frames 的图像长度声明；planes 默认写 1（同 rc），NSIS 风格传 0（原样拷 .ico 目录项）。
    slots 给出时用作各 RT_ICON 的实际字节（模拟 NSIS 共用槽的尾部填充或被截断的资源）。"""
    if slots is not None and len(slots) != len(frames):
        raise ValueError("slots 数量必须与 frames 一致")
    # 先组资源目录（RVA 暂按 0 写，稍后整体加上节 RVA）
    # 布局（全部相对资源节起点）：
    #   root dir + 2 id entries (type 3, 14)
    #   type 3 dir + N id entries
    #   type 14 dir + 1 id entry
    #   N lang dirs for icons + 1 lang dir for group (each 16+8)
    #   N+1 IMAGE_RESOURCE_DATA_ENTRY
    #   icon blobs + group blob
    n = len(frames)
    group = struct.pack("<HHH", 0, 1, n)
    for i, (w, h, img) in enumerate(frames, start=1):
        group += struct.pack(
            "<BBBBHHIH",
            0 if w >= 256 else w,
            0 if h >= 256 else h,
            0,
            0,
            planes,
            32,
            len(img),
            i,
        )

    def align4(x: int) -> int:
        return (x + 3) & ~3

    # 固定树尺寸
    root = 16 + 8 * 2
    type3 = 16 + 8 * n
    type14 = 16 + 8 * 1
    lang_size = (n + 1) * (16 + 8)
    data_entries = (n + 1) * 16
    tree_size = root + type3 + type14 + lang_size + data_entries

    blobs = []
    cursor = align4(tree_size)
    for idx, (_w, _h, img) in enumerate(frames):
        if slots is not None:
            img = slots[idx]
        blobs.append((cursor, img))
        cursor = align4(cursor + len(img))
    group_off = cursor
    blobs.append((group_off, group))
    cursor = align4(cursor + len(group))
    raw_res_size = cursor

    # 文件布局
    file_align = 0x200
    sect_align = 0x1000
    e_lfanew = 0x80
    opt_size = 0xE0  # PE32, 16 data dirs
    # headers: dos(e_lfanew) + PE+COFF+opt + 1 section
    headers_unaligned = e_lfanew + 4 + 20 + opt_size + 40
    size_of_headers = (headers_unaligned + file_align - 1) & ~(file_align - 1)
    raw_off = size_of_headers
    raw_size = (raw_res_size + file_align - 1) & ~(file_align - 1)
    res_rva = sect_align
    size_of_image = res_rva + ((raw_res_size + sect_align - 1) & ~(sect_align - 1))

    def pack_dir(named, ids):
        return struct.pack("<IIHHHH", 0, 0, 0, 0, named, ids)

    def pack_id_sub(id_, rel_off):
        return struct.pack("<II", id_, 0x80000000 | rel_off)

    def pack_id_data(id_, rel_off):
        return struct.pack("<II", id_, rel_off)

    # 偏移
    off_type3 = root
    off_type14 = off_type3 + type3
    off_langs = off_type14 + type14
    # lang dirs: first n for icons, last for group
    lang_dir_offs = [off_langs + i * 24 for i in range(n + 1)]
    off_data_entries = off_langs + lang_size
    data_entry_offs = [off_data_entries + i * 16 for i in range(n + 1)]

    buf = bytearray()
    # root
    buf += pack_dir(0, 2)
    buf += pack_id_sub(RT_ICON, off_type3)
    buf += pack_id_sub(RT_GROUP_ICON, off_type14)
    # type 3
    buf += pack_dir(0, n)
    for i in range(n):
        buf += pack_id_sub(i + 1, lang_dir_offs[i])
    # type 14
    buf += pack_dir(0, 1)
    buf += pack_id_sub(group_id, lang_dir_offs[n])
    # lang dirs (name=0, id=1, entry points to data entry)
    for i in range(n + 1):
        buf += pack_dir(0, 1)
        buf += pack_id_data(1033, data_entry_offs[i])
    # data entries (OffsetToData is RVA)
    for i, (blob_off, img) in enumerate(blobs):
        buf += struct.pack("<IIII", res_rva + blob_off, len(img), 0, 0)
    if len(buf) > tree_size:
        raise RuntimeError(f"resource tree overflow {len(buf)} > {tree_size}")
    buf += b"\0" * (tree_size - len(buf))
    # blobs at their offsets
    if len(buf) < raw_res_size:
        buf += b"\0" * (raw_res_size - len(buf))
    for blob_off, img in blobs:
        buf[blob_off : blob_off + len(img)] = img
    rsrc = bytes(buf)
    if len(rsrc) != raw_res_size:
        rsrc = rsrc[:raw_res_size].ljust(raw_res_size, b"\0")

    # DOS + PE
    dos = bytearray(e_lfanew)
    dos[0:2] = b"MZ"
    struct.pack_into("<I", dos, 0x3C, e_lfanew)

    pe = bytearray()
    pe += dos
    pe += b"PE\0\0"
    # COFF: i386, 1 section, opt 0xE0, executable + 32bit
    pe += struct.pack("<HHIIIHH", 0x14C, 1, 0, 0, 0, opt_size, 0x0102)
    # Optional: PE32
    opt = bytearray(opt_size)
    struct.pack_into("<H", opt, 0, 0x10B)  # magic
    struct.pack_into("<I", opt, 16, 0x1000)  # AddressOfEntryPoint (dummy)
    struct.pack_into("<I", opt, 28, 0x400000)  # ImageBase
    struct.pack_into("<I", opt, 32, sect_align)
    struct.pack_into("<I", opt, 36, file_align)
    struct.pack_into("<H", opt, 40, 6)  # MajorOperatingSystemVersion
    struct.pack_into("<H", opt, 48, 6)  # MajorSubsystemVersion
    struct.pack_into("<I", opt, 56, size_of_image)
    struct.pack_into("<I", opt, 60, size_of_headers)
    struct.pack_into("<H", opt, 68, 2)  # Subsystem WINDOWS_GUI
    struct.pack_into("<I", opt, 72, 0x100000)  # SizeOfStackReserve
    struct.pack_into("<I", opt, 76, 0x1000)  # SizeOfStackCommit
    struct.pack_into("<I", opt, 80, 0x100000)  # SizeOfHeapReserve
    struct.pack_into("<I", opt, 84, 0x1000)  # SizeOfHeapCommit
    struct.pack_into("<I", opt, 92, 16)  # NumberOfRvaAndSizes
    # data directory 2 = resources
    struct.pack_into("<II", opt, 96 + 16, res_rva, raw_res_size)
    pe += opt
    # section .rsrc
    name = b".rsrc\0\0\0"
    pe += name
    pe += struct.pack(
        "<IIIIIIHHI",
        raw_res_size,  # VirtualSize
        res_rva,  # VirtualAddress
        raw_size,  # SizeOfRawData
        raw_off,  # PointerToRawData
        0,
        0,
        0,
        0,
        0x40000040,  # INITIALIZED_DATA | READ
    )
    if len(pe) > size_of_headers:
        raise RuntimeError("headers too large")
    pe += b"\0" * (size_of_headers - len(pe))
    pe += rsrc.ljust(raw_size, b"\0")
    return bytes(pe)


def _rsrc_raw_offset(pe: bytes) -> int:
    """自测用：取 make_pe_with_icons 生成的 PE 里 .rsrc 节的文件偏移（单节）。"""
    e_lfanew = struct.unpack_from("<I", pe, 0x3C)[0]
    optsz = struct.unpack_from("<H", pe, e_lfanew + 4 + 16)[0]
    sec = e_lfanew + 4 + 20 + optsz
    return struct.unpack_from("<I", pe, sec + 20)[0]


def _git_show_bytes(spec: str) -> bytes | None:
    """自测用：读仓库里某个版本的文件；不在 git 仓库或没有该 tag 时返回 None（只影响本地附加对照）。"""
    import subprocess

    try:
        r = subprocess.run(["git", "-C", str(ROOT), "show", spec], capture_output=True, check=False)
    except OSError:
        return None
    return r.stdout if r.returncode == 0 and r.stdout else None


def write_temp(dirpath: Path, name: str, data: bytes) -> Path:
    p = dirpath / name
    p.write_bytes(data)
    return p


def write_conf(dirpath: Path, *, installer_icon: str | None, ico_rel: str = "icons/icon.ico") -> tuple[Path, Path]:
    icons_dir = dirpath / "icons"
    icons_dir.mkdir(exist_ok=True)
    conf = {
        "bundle": {
            "icon": ["icons/32x32.png", ico_rel],
            "windows": {"nsis": {}},
        }
    }
    if installer_icon is not None:
        conf["bundle"]["windows"]["nsis"]["installerIcon"] = installer_icon
    conf_path = dirpath / "tauri.conf.json"
    win_path = dirpath / "tauri.windows.conf.json"
    conf_path.write_text(json.dumps(conf), encoding="utf-8")
    # windows overlay may or may not repeat installerIcon; leave minimal
    win_path.write_text(json.dumps({"bundle": {"windows": {"nsis": {}}}}), encoding="utf-8")
    return conf_path, win_path


def self_test() -> int:
    fail = 0

    def expect(ok: bool, label: str, detail: str = ""):
        nonlocal fail
        if ok:
            print(f"ok   {label}")
        else:
            print(f"FAIL {label}" + (f"：{detail}" if detail else ""))
            fail = 1

    def run_quiet(*args, **kwargs) -> int:
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf), contextlib.redirect_stderr(buf):
            return run_check(*args, **kwargs)

    png16 = make_png(16, 16, b"\x11\x22\x33\xff")
    png32 = make_png(32, 32, b"\x44\x55\x66\xff")
    png24 = make_png(24, 24, b"\x77\x88\x99\xff")
    ico = make_ico([(16, 16, png16), (32, 32, png32)])
    pe = make_pe_with_icons([(16, 16, png16), (32, 32, png32)], group_id=32512)

    # 生成的 PE 必须能被解析，并与 ICO 一致
    groups = parse_pe_icon_groups(pe, "self-pe")
    ico_frames = parse_ico(ico, "self-ico")
    expect(len(groups) == 1 and len(groups[0]) == 2, "生成的 PE 含 1 个图标组、2 帧")
    expect(not compare_frames(groups[0], ico_frames, "pe", "ico"), "生成的 PE 与 ICO 字节一致")

    # 真实 v0.3.0 产物（若本机有下载）也作为对照，确认解析器对 PE32+ / PE32 都成立
    real_exe = Path("/tmp/ca-031-assets/portable/clash-assist.exe")
    real_setup = Path("/tmp/ca-031-assets/Clash.Assist_0.3.0_x64-setup.exe")
    real_ico = ROOT / "src-tauri" / "icons" / "icon.ico"
    real_ico_030 = _git_show_bytes("v0.3.0:src-tauri/icons/icon.ico")
    if real_exe.is_file() and real_ico_030 is not None:
        # 0.3.1 原位替换了 icon.ico，所以真实 v0.3.0 exe 要对照 v0.3.0 那份 .ico
        with tempfile.TemporaryDirectory(prefix="ca-icon-030-") as t030:
            ico030 = write_temp(Path(t030), "icon.ico", real_ico_030)
            errs = check_pe_against_ico(real_exe, ico030, "自测真实 exe")
        expect(not errs, "真实 v0.3.0 clash-assist.exe 与 v0.3.0 的 icon.ico 一致", "\n".join(errs))
    else:
        print("skip 真实 clash-assist.exe 对照（本地没有该产物，CI 自测不依赖它）")

    with tempfile.TemporaryDirectory(prefix="ca-icon-selftest-") as tmp:
        td = Path(tmp)
        (td / "icons").mkdir(exist_ok=True)
        write_temp(td / "icons", "icon.ico", ico)
        pe_path = write_temp(td, "app.exe", pe)
        other_pe = write_temp(
            td,
            "default-setup.exe",
            make_pe_with_icons([(16, 16, png24)], group_id=103),
        )
        wrong_pe = write_temp(
            td,
            "wrong.exe",
            make_pe_with_icons([(16, 16, png16), (32, 32, png24)], group_id=1),
        )
        missing_size_pe = write_temp(
            td,
            "missing.exe",
            make_pe_with_icons([(16, 16, png16)], group_id=1),
        )
        # NSIS 风格安装器：目录项原样取自 .ico（planes=0），组 id 103（NSIS IDI_ICON2）
        nsis_frames = [(16, 16, png16), (32, 32, png32)]
        nsis_pe = write_temp(td, "nsis-setup.exe", make_pe_with_icons(nsis_frames, group_id=103, planes=0))

        # 1) exe 对照通过
        conf, win = write_conf(td, installer_icon=None)
        rc = run_quiet(pe_path, None, conf, win)
        expect(rc == 0, "未配置 installerIcon 时，匹配的应用 exe 通过")

        # 2) 图像字节不对失败
        rc = run_quiet(wrong_pe, None, conf, win)
        expect(rc == 1, "应用 exe 图像字节不一致时失败")

        # 3) 尺寸集合缺一帧失败
        rc = run_quiet(missing_size_pe, None, conf, win)
        expect(rc == 1, "应用 exe 缺少尺寸时失败")

        # 4) 未配置 installerIcon + 默认安装器：不失败，不记通过（看返回码即可；日志含「不记为通过」）
        rc = run_quiet(pe_path, other_pe, conf, win)
        expect(rc == 0, "未配置 installerIcon 时，即使安装器图标不同也不因此失败")

        # 4b) 未配置、但安装器碰巧与 icon.ico 一致：仍然返回 0（不把这当成配置已生效的通过）
        rc = run_quiet(pe_path, pe_path, conf, win)
        expect(rc == 0, "未配置 installerIcon 且安装器碰巧一致时仍返回 0（不记通过）")

        # 5) 配置了 installerIcon，安装器匹配 → 通过
        conf2, win2 = write_conf(td, installer_icon="icons/icon.ico")
        rc = run_quiet(pe_path, nsis_pe, conf2, win2)
        expect(rc == 0, "配置了 installerIcon 且安装器与其一致时通过")

        # 6) 配置了 installerIcon，安装器是默认图标 → 失败
        rc = run_quiet(pe_path, other_pe, conf2, win2)
        expect(rc == 1, "配置了 installerIcon 但安装器仍是另一套图标时失败")

        # 7) 配置了 installerIcon，路径不存在 → 失败
        conf3, win3 = write_conf(td, installer_icon="icons/missing.ico")
        # write_conf 只写 JSON，不创建 missing.ico
        rc = run_quiet(pe_path, pe_path, conf3, win3)
        expect(rc == 1, "installerIcon 指向不存在的文件时失败")

        # 8) installerIcon 放在 tauri.windows.conf.json 里（与 Pit 的 win/031-icon 一致）也能读到
        conf4 = td / "base.json"
        win4 = td / "win.json"
        conf4.write_text(
            json.dumps({"bundle": {"icon": ["icons/icon.ico"]}}),
            encoding="utf-8",
        )
        win4.write_text(
            json.dumps({"bundle": {"windows": {"nsis": {"installerIcon": "icons/icon.ico"}}}}),
            encoding="utf-8",
        )
        rc = run_quiet(pe_path, other_pe, conf4, win4)
        expect(rc == 1, "installerIcon 只写在 windows 配置且产物不对时失败")
        rc = run_quiet(pe_path, nsis_pe, conf4, win4)
        expect(rc == 0, "installerIcon 只写在 windows 配置且产物一致时通过")

        # 8b) NSIS 共用图标槽（Source/icon.cpp set_main_icon）：槽长取安装器 / 卸载器两帧的较大者，
        #     先 memset 0 再拷安装器那帧，所以 RT_ICON 可以比组目录声明的长，但尾部只能是 0。
        def nsis_variant(fname: str, slot16: bytes) -> Path:
            return write_temp(
                td, fname, make_pe_with_icons(nsis_frames, group_id=103, planes=0, slots=[slot16, png32])
            )

        def inst_errs(pe_file: Path, *, nsis: bool = True) -> str:
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                return "\n".join(check_pe_against_ico(pe_file, td / "icons" / "icon.ico", "安装器", nsis=nsis))

        zero_pad = nsis_variant("nsis-zero-pad.exe", png16 + b"\0" * 344)
        rc = run_quiet(pe_path, zero_pad, conf4, win4)
        expect(rc == 0, "安装器 RT_ICON 尾部全零填充（NSIS 共用槽）时通过")
        frames = parse_pe_icon_groups(zero_pad.read_bytes(), "zero-pad", nsis_zero_padding=True)[0]
        expect(
            [f["pad"] for f in frames] == [344, 0] and frames[0]["data"] == png16,
            "全零填充只截掉尾部、帧数据就是声明长度的原图",
        )
        rc = run_quiet(zero_pad, None, conf4, win4)
        e = inst_errs(zero_pad, nsis=False)
        expect(rc == 1 and "与组目录声明的 82 不一致" in e, "应用 exe 的 RT_ICON 带同样的全零尾部时仍失败（应用 exe 不放宽）", e)

        garbage_pad = nsis_variant("nsis-garbage-pad.exe", png16 + b"\0" * 343 + b"\x01")
        rc = run_quiet(pe_path, garbage_pad, conf4, win4)
        e = inst_errs(garbage_pad)
        expect(rc == 1 and "不全是 0" in e, "安装器 RT_ICON 尾部有非零字节时失败", e)
        leftover_pad = nsis_variant("nsis-leftover-pad.exe", png16 + png24[:344])
        rc = run_quiet(pe_path, leftover_pad, conf4, win4)
        e = inst_errs(leftover_pad)
        expect(rc == 1 and "不全是 0" in e, "安装器 RT_ICON 尾部是另一帧残留字节时失败", e)
        truncated = nsis_variant("nsis-truncated.exe", png16[:-1])
        rc = run_quiet(pe_path, truncated, conf4, win4)
        e = inst_errs(truncated)
        expect(rc == 1 and "大小 81 与组目录声明的 82 不一致" in e, "安装器 RT_ICON 比声明的短时失败", e)

        # 8c) 安装器的组目录项必须与 .ico 逐字段一致（NSIS 原样拷目录项）；rc 风格 planes=1 也要拒绝
        rc = run_quiet(pe_path, pe_path, conf4, win4)
        e = inst_errs(pe_path)
        expect(rc == 1 and "目录项 (16, 16, 0, 0, 1, 32, 82)" in e, "安装器组目录项 planes 与 .ico 不一致时失败", e)
        extra_pe = write_temp(
            td,
            "nsis-extra.exe",
            make_pe_with_icons(nsis_frames + [(24, 24, png24)], group_id=103, planes=0),
        )
        rc = run_quiet(pe_path, extra_pe, conf4, win4)
        expect(rc == 1 and "尺寸集合" in inst_errs(extra_pe), "安装器多出一帧时失败")
        nsis_missing = write_temp(td, "nsis-missing.exe", make_pe_with_icons(nsis_frames[:1], group_id=103, planes=0))
        rc = run_quiet(pe_path, nsis_missing, conf4, win4)
        expect(rc == 1 and "尺寸集合" in inst_errs(nsis_missing), "安装器少一帧时失败")

        # 9) 不是 PE 的文件
        junk = write_temp(td, "junk.exe", b"not a pe")
        rc = run_quiet(junk, None, conf, win)
        expect(rc == 1, "应用 exe 不是 PE 时失败")

        # 9b) 截断的 PE（COFF 头不完整）：struct.error 必须收成检查失败，而不是 traceback
        trunc_pe = write_temp(td, "trunc.exe", pe[: 0x80 + 4 + 10])
        try:
            parse_pe_icon_groups(trunc_pe.read_bytes(), "trunc")
            expect(False, "截断 PE 解析应抛 ValueError")
        except ValueError:
            expect(True, "截断 PE（COFF 头不完整）解析抛 ValueError 而不是 struct.error")
        except Exception as e:  # noqa: BLE001 - 自测要看清具体异常类型
            expect(False, "截断 PE 解析应抛 ValueError", f"{type(e).__name__}: {e}")
        rc = run_quiet(trunc_pe, None, conf, win)
        expect(rc == 1, "应用 exe 是截断 PE 时失败（退出码 1）")
        rc = run_quiet(pe_path, trunc_pe, conf, win)
        expect(rc == 1, "未配置 installerIcon、安装器是截断 PE 时报告失败（退出码 1）")
        trunc_opt = write_temp(td, "trunc-opt.exe", pe[: 0x80 + 4 + 20 + 50])
        rc = run_quiet(trunc_opt, None, conf, win)
        expect(rc == 1, "可选头被截断（数据目录读不到）时失败")

        # 9c) 截断的 ICO（目录项不完整）：同样记为失败
        tdi = td / "truncico"
        (tdi / "icons").mkdir(parents=True)
        write_temp(tdi / "icons", "icon.ico", ico[:10])
        conf_t, win_t = write_conf(tdi, installer_icon=None)
        rc = run_quiet(pe_path, None, conf_t, win_t)
        expect(rc == 1, "icon.ico 截断时失败")

        # 9d) 资源目录成环：根目录的子目录指回根目录
        rsrc_off = _rsrc_raw_offset(pe)
        loop_pe = bytearray(pe)
        struct.pack_into("<I", loop_pe, rsrc_off + 16 + 4, 0x80000000 | 0)
        loop_path = write_temp(td, "loop.exe", bytes(loop_pe))
        try:
            parse_pe_icon_groups(bytes(loop_pe), "loop")
            expect(False, "资源目录成环应被拒绝")
        except ValueError as e:
            expect("成环" in str(e) or "嵌套" in str(e), "资源目录成环被拒绝（ValueError）", str(e))
        except RecursionError:
            expect(False, "资源目录成环应被拒绝", "RecursionError")
        rc = run_quiet(loop_path, None, conf, win)
        expect(rc == 1, "应用 exe 资源目录成环时失败")

        # 9e) 资源目录过深（无环）：语言目录的项再指向另一个语言目录
        n_frames = 2
        off_langs = (16 + 8 * 2) + (16 + 8 * n_frames) + (16 + 8)
        deep_pe = bytearray(pe)
        struct.pack_into("<I", deep_pe, rsrc_off + off_langs + 16 + 4, 0x80000000 | (off_langs + 24))
        deep_path = write_temp(td, "deep.exe", bytes(deep_pe))
        try:
            parse_pe_icon_groups(bytes(deep_pe), "deep")
            expect(False, "资源目录过深应被拒绝")
        except ValueError as e:
            expect("嵌套" in str(e), "资源目录超过 3 层被拒绝（ValueError）", str(e))
        rc = run_quiet(deep_path, None, conf, win)
        expect(rc == 1, "应用 exe 资源目录过深时失败")

        # 10) 真实 v0.3.0 安装器（未配置 installerIcon 时构建）：未配置时只报告默认且不失败
        if real_setup.is_file():
            # 用不含 installerIcon 的临时配置（仓库配置从 0.3.1 起已配置 installerIcon；
            # td 下的 tauri.conf.json 已被第 7 步改写，这里另建目录）
            tdn = td / "noinst"
            tdn.mkdir()
            (tdn / "icons").mkdir()
            write_temp(tdn / "icons", "icon.ico", ico)
            conf_n, win_n = write_conf(tdn, installer_icon=None)
            rc = run_quiet(None, real_setup, conf_n, win_n)
            expect(rc == 0, "真实 v0.3.0 setup.exe 在未配置 installerIcon 时只报告默认、不失败")
            # 若伪造「已配置」，对照当前 icon.ico 必须失败（默认 NSIS 图标对不上）
            conf_fake = td / "fake-base.json"
            win_fake = td / "fake-win.json"
            conf_fake.write_text(json.dumps({"bundle": {"icon": ["icons/icon.ico"]}}), encoding="utf-8")
            win_fake.write_text(
                json.dumps({"bundle": {"windows": {"nsis": {"installerIcon": "icons/icon.ico"}}}}),
                encoding="utf-8",
            )
            # icon path is relative to conf dir, so copy ico next to fake conf
            (td / "icons").mkdir(exist_ok=True)
            if not (td / "icons" / "icon.ico").exists():
                (td / "icons" / "icon.ico").write_bytes(real_ico.read_bytes())
            else:
                # already have test ico; write a second tree
                pass
            # Use a subdir with a copy of the real icon
            sub = td / "realcfg"
            (sub / "icons").mkdir(parents=True, exist_ok=True)
            (sub / "icons" / "icon.ico").write_bytes(real_ico.read_bytes())
            (sub / "tauri.conf.json").write_text(
                json.dumps({"bundle": {"icon": ["icons/icon.ico"]}}), encoding="utf-8"
            )
            (sub / "tauri.windows.conf.json").write_text(
                json.dumps({"bundle": {"windows": {"nsis": {"installerIcon": "icons/icon.ico"}}}}),
                encoding="utf-8",
            )
            rc = run_quiet(None, real_setup, sub / "tauri.conf.json", sub / "tauri.windows.conf.json")
            expect(rc == 1, "伪造已配置 installerIcon 时，v0.3.0 默认安装器图标必须被拒绝")

    print("check-windows-icons 自测：" + ("全部通过" if not fail else "失败"))
    return fail


def main(argv: list[str]) -> int:
    _ensure_utf8_stdio()
    p = argparse.ArgumentParser(description="检查 Windows exe / NSIS 安装器的图标资源")
    p.add_argument("--self-test", action="store_true", help="运行自测后退出")
    p.add_argument("--exe", type=Path, help="应用 exe（clash-assist.exe）")
    p.add_argument("--installer", type=Path, help="NSIS 安装器 setup.exe")
    p.add_argument("--config", type=Path, default=DEFAULT_CONF, help="tauri.conf.json")
    p.add_argument("--windows-config", type=Path, default=DEFAULT_WIN_CONF, help="tauri.windows.conf.json")
    args = p.parse_args(argv)
    if args.self_test:
        if args.exe or args.installer:
            print("check-windows-icons：--self-test 不能与 --exe / --installer 同时使用", file=sys.stderr)
            return 2
        return self_test()
    if args.exe is None and args.installer is None:
        print("用法：check-windows-icons.py --exe <app.exe> [--installer <setup.exe>] | --self-test", file=sys.stderr)
        return 2
    return run_check(args.exe, args.installer, args.config, args.windows_config)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
