/** Render the real UpdatePill JSX (no DOM): all locale copy, copyable checked URL, and copy action. */
import { afterEach, describe, expect, it, vi } from "vitest";
import { isValidElement, type ReactNode } from "react";

const h = vi.hoisted(() => ({ t: (key: string): string => key }));
vi.mock("../i18n", () => ({ useI18n: () => ({ t: h.t }) }));

import { UpdatePill } from "./TitleBar";
import { initialUpdateFlowState, updateViewOf, type UpdateFlowState } from "../hooks/useUpdateFlow";
import { zhHans } from "../i18n/locales/zh-Hans";
import { zhHant } from "../i18n/locales/zh-Hant";
import { en } from "../i18n/locales/en";

const URL = "https://github.com/JTee77/clash-assist/releases/tag/v0.3.1";
const ERROR = "安装程序退出码 42（可能是降级被拒绝或被系统拦截），未完成更新";
const LANGUAGES = [
  ["zh-Hans", zhHans, "已为你打开下载页", "未能自动打开下载页", "复制链接"],
  ["zh-Hant", zhHant, "已為你開啟下載頁", "未能自動開啟下載頁", "複製連結"],
  ["en", en, "Opened download page", "Couldn’t open the download page", "Copy link"],
] as const;

type NodeProps = { children?: ReactNode; className?: string; title?: string; onClick?: () => void };
function elements(node: ReactNode): NodeProps[] {
  if (Array.isArray(node)) return node.flatMap(elements);
  if (!isValidElement<NodeProps>(node)) return [];
  return [node.props, ...elements(node.props.children)];
}
function text(node: ReactNode): string {
  if (Array.isArray(node)) return node.map(text).join("");
  if (typeof node === "string" || typeof node === "number") return String(node);
  return isValidElement<NodeProps>(node) ? text(node.props.children) : "";
}
function render(releaseOpen: UpdateFlowState["releaseOpen"], url = URL) {
  const state: UpdateFlowState = {
    ...initialUpdateFlowState,
    stage: "fallback",
    releaseOpen,
    error: ERROR,
    update: {
      current: "0.3.0", latest: "0.3.1", update_available: true,
      install_kind: "nsis", one_click: true, url,
    },
  };
  return UpdatePill({ flow: {
    state, view: updateViewOf(state, { windows: true, oneClick: true }),
    primary: async () => {}, openRelease: () => {}, dismiss: () => {},
  } });
}

afterEach(() => vi.unstubAllGlobals());

describe.each(LANGUAGES)("UpdatePill 回退文案（%s）", (_lang, messages, opened, failed, copy) => {
  it("命令成功才显示成功文案，不展示 URL / 复制链接按钮", () => {
    h.t = (key) => messages[key as keyof typeof messages];
    const jsx = render("opened");
    expect(text(jsx)).toContain(opened);
    expect(text(jsx)).not.toContain(failed);
    expect(text(jsx)).not.toContain(URL);
    expect(elements(jsx).find((p) => p.className === "update-pill-url")).toBeUndefined();
    expect(elements(jsx).find((p) => text(p.children) === copy)).toBeUndefined();
    expect(jsx?.props.title).toBe(ERROR);
  });

  it("命令失败显示失败文案与检查结果中的可复制 URL，绝不显示成功文案", async () => {
    h.t = (key) => messages[key as keyof typeof messages];
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.stubGlobal("navigator", { clipboard: { writeText } });
    const jsx = render("failed");
    expect(text(jsx)).toContain(failed);
    expect(text(jsx)).not.toContain(opened);
    expect(text(jsx)).toContain(URL);
    const address = elements(jsx).find((p) => p.className === "update-pill-url");
    expect(address?.children).toBe(URL);
    expect(address?.title).toBe(URL);
    const button = elements(jsx).find((p) => text(p.children) === copy && p.onClick);
    expect(button).toBeDefined();
    button!.onClick!();
    await Promise.resolve();
    expect(writeText).toHaveBeenCalledExactlyOnceWith(URL);
    expect(jsx?.props.title).toBe(ERROR);
  });

  it("不展示任意 URL；剪贴板不可用时地址仍是可选中的文本", async () => {
    h.t = (key) => messages[key as keyof typeof messages];
    const bad = render("failed", "javascript:alert(1)");
    expect(text(bad)).not.toContain("javascript:");
    expect(elements(bad).find((p) => p.className === "update-pill-url")).toBeUndefined();
    vi.stubGlobal("navigator", { clipboard: { writeText: () => Promise.reject("blocked") } });
    const jsx = render("failed");
    const button = elements(jsx).find((p) => text(p.children) === copy && p.onClick);
    button!.onClick!();
    await new Promise((r) => setTimeout(r, 0));
    expect(text(jsx)).toContain(URL);
  });

  it("请求进行中不宣称已打开或失败", () => {
    h.t = (key) => messages[key as keyof typeof messages];
    const jsx = render("opening");
    expect(text(jsx)).toContain(messages["update.fallbackOpening"]);
    expect(text(jsx)).not.toContain(opened);
    expect(text(jsx)).not.toContain(failed);
    expect(text(jsx)).not.toContain(URL);
  });
});
