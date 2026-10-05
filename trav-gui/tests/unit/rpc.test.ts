import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const store = new Map<string, string>();

beforeEach(() => {
  store.clear();
  vi.stubGlobal("window", { location: { search: "" } });
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => void store.set(k, v),
  });
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.resetModules();
});

function mockFetch(status: number, body: unknown) {
  const fn = vi.fn(async (_url: string, _init: RequestInit) => ({ status, json: async () => body }));
  vi.stubGlobal("fetch", fn);
  return fn;
}

describe("rpc transport (HTTP)", () => {
  it("posts method + params and unwraps results", async () => {
    const f = mockFetch(200, { ok: true, result: { seq: 7 } });
    const { call } = await import("@/lib/rpc");
    await expect(call("snapshot")).resolves.toEqual({ seq: 7 });
    const [url, init] = f.mock.calls[0];
    expect(url).toBe("/api/rpc");
    expect(JSON.parse(String(init.body))).toEqual({ method: "snapshot", params: null });
    expect((init.headers as Record<string, string>).authorization).toBeUndefined();
  });

  it("sends the stored token and maps 401 to AuthError", async () => {
    const f = mockFetch(401, { ok: false, error: "unauthorized" });
    const { call, setToken, AuthError } = await import("@/lib/rpc");
    setToken("s3cret");
    await expect(call("snapshot")).rejects.toBeInstanceOf(AuthError);
    expect((f.mock.calls[0][1].headers as Record<string, string>).authorization).toBe("Bearer s3cret");
  });

  it("surfaces engine errors", async () => {
    mockFetch(400, { ok: false, error: "torrent is already in the list" });
    const { call } = await import("@/lib/rpc");
    await expect(call("add", { magnet: "x" })).rejects.toThrow("already in the list");
  });

  it("reads a token from the URL once and persists it", async () => {
    vi.stubGlobal("window", { location: { search: "?token=fromurl" } });
    const { getToken } = await import("@/lib/rpc");
    expect(getToken()).toBe("fromurl");
    expect(store.get("trav.token")).toBe("fromurl");
  });

  it("is not desktop without Tauri internals", async () => {
    const { isDesktop } = await import("@/lib/rpc");
    expect(isDesktop()).toBe(false);
  });
});
