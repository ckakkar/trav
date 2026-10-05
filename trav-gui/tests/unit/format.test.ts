import { describe, expect, it } from "vitest";
import { bytes, bytesParts, eta, looksLikeMagnet, parseRate, pct, rate } from "@/lib/format";

describe("format", () => {
  it("bytes uses 1024 steps with short units", () => {
    expect(bytes(0)).toBe("0 B");
    expect(bytes(-5)).toBe("0 B");
    expect(bytes(NaN)).toBe("0 B");
    expect(bytes(512)).toBe("512 B");
    expect(bytes(1536)).toBe("1.5 KB");
    expect(bytes(150 * 1024 ** 2)).toBe("150 MB");
    expect(bytes(1024 ** 6)).toBe("1024 PB");
    expect(bytesParts(1536)).toEqual(["1.5", "KB"]);
  });

  it("rate shows an em dash at zero", () => {
    expect(rate(0)).toBe("—");
    expect(rate(2048)).toBe("2.0 KB/s");
  });

  it("eta covers every magnitude", () => {
    expect(eta(null)).toBe("∞");
    expect(eta(Infinity)).toBe("∞");
    expect(eta(86400 * 400)).toBe("∞");
    expect(eta(42)).toBe("42s");
    expect(eta(125)).toBe("2m 05s");
    expect(eta(3 * 3600 + 7 * 60)).toBe("3h 07m");
    expect(eta(2 * 86400 + 5 * 3600)).toBe("2d 5h");
  });

  it("pct clamps", () => {
    expect(pct(-1)).toBe("0.0%");
    expect(pct(0.1234)).toBe("12.3%");
    expect(pct(1.5)).toBe("100%");
  });

  it("parseRate understands units", () => {
    expect(parseRate("800k")).toBe(800 * 1024);
    expect(parseRate("1.5 MB/s")).toBe(Math.round(1.5 * 1024 ** 2));
    expect(parseRate("2g")).toBe(2 * 1024 ** 3);
    expect(parseRate("nope")).toBe(0);
  });

  it("recognises magnets and bare info-hashes", () => {
    expect(looksLikeMagnet("magnet:?xt=urn:btih:abc")).toBe(true);
    expect(looksLikeMagnet("  " + "A1".repeat(20) + " ")).toBe(true);
    expect(looksLikeMagnet("https://example.com")).toBe(false);
    expect(looksLikeMagnet("a1".repeat(19))).toBe(false);
  });
});
