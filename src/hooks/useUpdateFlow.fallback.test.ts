/**
 * 前端回退契约（docs/windows-updater.md「前端回退契约（自动化分层测试）」）：驱动**真实的** useUpdateFlow
 * （真实 reducer、primary / fail 编排、lib/update 的 invoke 桥接），只把 Tauri invoke / listen 换成 mock。
 *
 * 测试栈只有 vitest（node 环境，无 DOM、无 @testing-library），所以用一个最小的 React hooks 运行时
 * 替换 "react" 的 useReducer / useEffect：按调用顺序存槽位、依赖变化时运行 effect 并保留 cleanup、
 * dispatch 后重新渲染。hook 本身的代码一行不改。
 *
 * 证明范围：Rust 命令以字符串 reject（installer_exit_nonzero 的说明文字，形状由 Rust 单测
 * relaunch_error_reaches_frontend_as_plain_string 固定）时，前端恰好调用一次 open_release_page(检查结果里的 url)，
 * 停在 fallback、保留错误、不进入 restarting / updated；open_release_page 自身失败也不产生未处理的 rejection。
 * 不证明真实浏览器被打开（M6 实机测试）。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

type Slot = { state?: unknown; deps?: readonly unknown[]; cleanup?: void | (() => void) };

const rt = vi.hoisted(() => {
  const r = {
    slots: [] as Slot[],
    idx: 0,
    dirty: false,
    queue: [] as Array<() => void>,
  };
  return r;
});

vi.mock("react", () => ({
  useReducer<S, E>(reducer: (s: S, e: E) => S, init: S): [S, (e: E) => void] {
    const i = rt.idx++;
    if (!rt.slots[i]) rt.slots[i] = { state: init };
    const slot = rt.slots[i];
    const dispatch = (e: E) => {
      const next = reducer(slot.state as S, e);
      if (!Object.is(next, slot.state)) {
        slot.state = next;
        rt.dirty = true;
      }
    };
    return [slot.state as S, dispatch];
  },
  useEffect(fn: () => void | (() => void), deps?: readonly unknown[]) {
    const i = rt.idx++;
    const prev = rt.slots[i];
    const changed =
      !prev || !deps || !prev.deps || deps.length !== prev.deps.length || deps.some((d, k) => !Object.is(d, prev.deps![k]));
    if (!changed) return;
    rt.queue.push(() => {
      if (prev && typeof prev.cleanup === "function") prev.cleanup();
      rt.slots[i] = { deps, cleanup: fn() };
    });
  },
}));

// invoke 用普通函数而不是 vi.fn：vi.fn 会给返回的 Promise 挂 then 来记录 settledResults，
// 那样即使前端没处理 rejection 也不会触发 unhandledRejection，「无未处理 rejection」的断言就失效了。
const h = vi.hoisted(() => ({
  calls: [] as unknown[][],
  impl: (() => Promise.reject(new Error("invoke 未配置"))) as (cmd: string, args?: unknown) => Promise<unknown>,
}));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: unknown) => {
    h.calls.push(args === undefined ? [cmd] : [cmd, args]);
    return h.impl(cmd, args);
  },
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import { useUpdateFlow } from "./useUpdateFlow";
import { __resetUpdateCheckForTests, type UpdateInfo } from "../lib/update";

type Flow = ReturnType<typeof useUpdateFlow>;

/** 最小渲染器：渲染一次 → 运行排队的 effect；flush() 让出微任务 / 宏任务，有 dispatch 就重渲染，直到稳定。 */
function mount(windows: boolean) {
  let current!: Flow;
  const render = () => {
    rt.idx = 0;
    rt.dirty = false;
    current = useUpdateFlow({ windows });
    const q = rt.queue.splice(0);
    for (const run of q) run();
  };
  const flush = async () => {
    for (let i = 0; i < 50; i++) {
      await new Promise((r) => setTimeout(r, 0));
      if (rt.dirty) render();
      else if (i > 3) return;
    }
    throw new Error("渲染没有稳定下来");
  };
  render();
  return {
    get flow() {
      return current;
    },
    flush,
    unmount() {
      for (const s of rt.slots) if (s && typeof s.cleanup === "function") s.cleanup();
    },
  };
}

const RELEASE_URL = "https://github.com/JTee77/clash-assist/releases/tag/v0.3.1";
const INFO: UpdateInfo = {
  current: "0.3.0",
  latest: "0.3.1",
  update_available: true,
  url: RELEASE_URL,
  install_kind: "nsis",
  one_click: true,
};
/** Rust 端 installer_exit_nonzero 的说明文字（windows.rs launch_blocking；invoke 以这个字符串 reject）。 */
const EXIT_NONZERO = "安装程序退出码 42（可能是降级被拒绝或被系统拦截），未完成更新";

type Handlers = { relaunch: () => Promise<unknown>; openRelease: () => Promise<unknown> };
function wire(hs: Handlers) {
  h.impl = (cmd: string) => {
    switch (cmd) {
      case "check_update":
        return Promise.resolve(INFO);
      case "download_update":
        return Promise.resolve("C:\\Users\\me\\AppData\\Local\\ClashAssist\\updates\\x");
      case "relaunch_updated_app":
        return hs.relaunch();
      case "open_release_page":
        return hs.openRelease();
      default:
        return Promise.reject(new Error(`unexpected invoke ${cmd}`));
    }
  };
}
const calls = (cmd: string) => h.calls.filter((c) => c[0] === cmd);

/** 检查 → 一键更新（下载校验）→ 立即重启：返回点击重启后的 hook。 */
async function toRelaunch() {
  const m = mount(true);
  await m.flush();
  expect(m.flow.state.stage).toBe("idle");
  expect(m.flow.view.primary).toBe("update.install");
  await m.flow.primary();
  await m.flush();
  expect(m.flow.state.stage).toBe("ready");
  expect(m.flow.view.primary).toBe("update.relaunch");
  await m.flow.primary();
  await m.flush();
  return m;
}

describe("前端回退契约：relaunch 失败（installer_exit_nonzero）→ 打开发布页", () => {
  const unhandled: unknown[] = [];
  const onUnhandled = (e: unknown) => unhandled.push(e);
  const proc = (globalThis as unknown as { process: { on(e: string, f: (x: unknown) => void): void; off(e: string, f: (x: unknown) => void): void } }).process;

  beforeEach(() => {
    rt.slots = [];
    rt.queue = [];
    rt.idx = 0;
    rt.dirty = false;
    h.calls = [];
    __resetUpdateCheckForTests();
    (globalThis as unknown as { window?: Record<string, unknown> }).window = { __TAURI_INTERNALS__: {} };
    unhandled.length = 0;
    proc.on("unhandledRejection", onUnhandled);
  });
  afterEach(() => {
    proc.off("unhandledRejection", onUnhandled);
  });

  it("relaunch 以 installer_exit_nonzero 的字符串 reject：恰好一次 open_release_page(检查结果的 url)，停在 fallback 并保留错误", async () => {
    wire({ relaunch: () => Promise.reject(EXIT_NONZERO), openRelease: () => Promise.resolve(null) });
    const m = await toRelaunch();
    expect(calls("relaunch_updated_app")).toHaveLength(1);
    expect(calls("open_release_page")).toEqual([["open_release_page", { url: RELEASE_URL }]]);
    expect(m.flow.state.stage).toBe("fallback");
    expect(m.flow.state.error).toBe(EXIT_NONZERO);
    expect(m.flow.view).toMatchObject({
      visible: true,
      status: { key: "update.fallbackOpened" },
      primary: "update.go",
      showClose: true,
    });
    // 再等几轮也不会进入成功 / 退出状态，也不会再次打开发布页
    await m.flush();
    expect(m.flow.state.stage).toBe("fallback");
    expect(calls("open_release_page")).toHaveLength(1);
    // fallback 下主按钮 = 用户主动再次打开发布页（不是重新启动安装程序）
    await m.flow.primary();
    expect(calls("open_release_page")).toHaveLength(2);
    expect(calls("relaunch_updated_app")).toHaveLength(1);
    expect(unhandled).toEqual([]);
    m.unmount();
  });

  it("open_release_page 自身失败：没有未处理的 rejection，不算成功，installer 错误仍可见", async () => {
    wire({
      relaunch: () => Promise.reject(EXIT_NONZERO),
      openRelease: () => Promise.reject("只允许打开本项目的 release 页面"),
    });
    const m = await toRelaunch();
    await new Promise((r) => setTimeout(r, 20));
    await m.flush();
    expect(calls("open_release_page")).toEqual([["open_release_page", { url: RELEASE_URL }]]);
    expect(m.flow.state.stage).toBe("fallback");
    expect(m.flow.state.stage).not.toBe("restarting");
    expect(m.flow.state.error).toBe(EXIT_NONZERO);
    expect(m.flow.view.primary).toBe("update.go");
    expect(unhandled).toEqual([]);
    m.unmount();
  });

  it("Error 对象形式的 rejection 同样回退（errMsg 取 message）", async () => {
    wire({ relaunch: () => Promise.reject(new Error(EXIT_NONZERO)), openRelease: () => Promise.resolve(null) });
    const m = await toRelaunch();
    expect(m.flow.state).toMatchObject({ stage: "fallback", error: EXIT_NONZERO });
    expect(calls("open_release_page")).toEqual([["open_release_page", { url: RELEASE_URL }]]);
    m.unmount();
  });

  it("成功启动（relaunch resolve，进程随后退出）：不触发回退，不打开发布页", async () => {
    wire({ relaunch: () => Promise.resolve(null), openRelease: () => Promise.resolve(null) });
    const m = await toRelaunch();
    await m.flush();
    expect(calls("relaunch_updated_app")).toHaveLength(1);
    expect(calls("open_release_page")).toHaveLength(0);
    expect(m.flow.state.stage).toBe("restarting");
    expect(m.flow.state.error).toBeNull();
    expect(m.flow.view.primary).toBeNull();
    expect(unhandled).toEqual([]);
    m.unmount();
  });
});
