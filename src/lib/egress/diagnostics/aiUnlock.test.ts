import { describe, it, expect, vi, beforeEach } from "vitest";
import * as fetchVia from "../fetchVia";
import {
  extractGeminiRegion,
  probeGeminiUnlock,
  probeChatgptUnlock,
  unlockCard,
} from "./aiUnlock";

describe("extractGeminiRegion", () => {
  it("parses escaped and unescaped vXmutd country code", () => {
    expect(extractGeminiRegion('foo "vXmutd":"\\"US\\"" bar')).toBe("US");
    expect(extractGeminiRegion('foo "vXmutd":"JP" bar')).toBe("JP");
    expect(extractGeminiRegion('foo "vXmutd":"\\"ZZ\\"" bar')).toBeNull();
  });

  it("normalizes ISO3 codes from WIZ tuple", () => {
    expect(extractGeminiRegion('prefix ,2,1,200,"USA" suffix')).toBe("US");
    expect(extractGeminiRegion('prefix ,2,1,200,"HKG" suffix')).toBe("HK");
    expect(extractGeminiRegion('prefix ,2,1,200,"SGP" suffix')).toBe("SG");
  });
});

describe("probeGeminiUnlock", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it("recognizes region block text and FAQ redirect", async () => {
    const spy = vi.spyOn(fetchVia, "fetchTextViaProxy");
    spy.mockResolvedValueOnce({
      ok: true,
      status: 200,
      text: '<link rel="canonical" href="https://gemini.google.com/faq">' + "x".repeat(60000),
      via: "rust",
    });
    const r1 = await probeGeminiUnlock(7897);
    expect(r1.supported).toBe(false);
    expect(r1.status).toBe("不可用（地区限制）");

    spy.mockResolvedValueOnce({
      ok: true,
      status: 200,
      text: 'window.WIZ_global_data = {"vXmutd":"\\"HK\\""};' + "x".repeat(30000),
      via: "rust",
    });
    const r2 = await probeGeminiUnlock(7897);
    expect(r2.supported).toBe(false);
    expect(r2.status).toBe("不可用（地区限制）");
  });

  it("accepts medium-sized WIZ_global_data app shell without requiring >50KB", async () => {
    vi.spyOn(fetchVia, "fetchTextViaProxy").mockResolvedValueOnce({
      ok: true,
      status: 200,
      text: '<title>Gemini</title><script>window.WIZ_global_data={"vXmutd":"\\"US\\""};</script>' + "a".repeat(15000),
      via: "rust",
    });
    const r = await probeGeminiUnlock(7897);
    expect(r.supported).toBe(true);
    expect(r.region).toBe("US");
    expect(r.status).toBe("可用（US）");
    const c = unlockCard("gemini", "Gemini", r);
    expect(c.level).toBe("pass");
  });

  it("returns unknown on timeout / empty failure", async () => {
    vi.spyOn(fetchVia, "fetchTextViaProxy").mockResolvedValueOnce({
      ok: false,
      status: 0,
      text: "",
      via: "rust",
    });
    const r = await probeGeminiUnlock(7897);
    expect(r.supported).toBe(false);
    expect(r.level).toBe("unknown");
    expect(r.status).toBe("超时未响应");
  });
});

describe("probeChatgptUnlock", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it("parses cdn-cgi/trace loc and compliance response", async () => {
    const spy = vi.spyOn(fetchVia, "fetchTextViaProxy");
    spy.mockResolvedValueOnce({
      ok: true,
      status: 200,
      text: "fl=1\nloc=SG\n",
      via: "rust",
    });
    spy.mockResolvedValueOnce({
      ok: true,
      status: 200,
      text: '{"cookie_requirements":false}',
      via: "rust",
    });
    const r = await probeChatgptUnlock(7897);
    expect(r.supported).toBe(true);
    expect(r.region).toBe("SG");
    expect(r.status).toBe("可用（SG）");
  });
});
