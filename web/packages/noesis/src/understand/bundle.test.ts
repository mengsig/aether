import type { aether } from "@aether-ui/protocol";
import { describe, expect, test } from "vitest";
import { BUNDLE_LIMIT_BYTES, BUNDLE_MAX_ITEMS, bundleOf, checkBundle, evidence, parseUnderstand, plainText, trimToBytes, understandHref, utf8Bytes } from "./bundle.ts";

const item = (excerpt: string, status: aether.EvidenceStatus = "preliminary"): aether.EvidenceItem => ({ ref: { kind: "item", id: "qi_x" }, excerpt, status });
const sized = (...excerpts: string[]): aether.EvidenceBundle => ({ title: "Size", items: excerpts.map((text) => item(text)) });

describe("the 32 KiB limit, in UTF-8 bytes", () => {
  test("accepts exactly 32768 bytes across items", () => {
    const check = checkBundle(sized("a".repeat(16_384), "b".repeat(16_384)));
    expect(check).toEqual({ ok: true, bytes: 32_768 });
  });

  test("refuses 32769 bytes", () => {
    const check = checkBundle(sized("a".repeat(16_384), "b".repeat(16_385)));
    expect(check.ok).toBe(false);
    expect(check.bytes).toBe(32_769);
  });

  test("counts multi-byte characters by their bytes, not their string length", () => {
    // "é" is two bytes, "€" three: 16383 × 2 + 2 = 32768 accepted; 10923 × 3 = 32769 refused at 10923 characters.
    expect(checkBundle(sized("é".repeat(16_383), "é"))).toEqual({ ok: true, bytes: BUNDLE_LIMIT_BYTES });
    const euros = "€".repeat(10_923);
    expect(euros.length).toBeLessThan(BUNDLE_LIMIT_BYTES);
    expect(checkBundle(sized(euros))).toMatchObject({ ok: false, bytes: 32_769 });
  });

  test("refuses a bundle with nothing in it", () => {
    expect(checkBundle({ title: "Empty", items: [] })).toMatchObject({ ok: false, bytes: 0 });
  });
});

describe("the daemon's other bounds", () => {
  test("refuses more than 50 facts, as the daemon does", () => {
    const many = (count: number) => ({ title: "Many", items: Array.from({ length: count }, () => item("fact")) });
    expect(checkBundle(many(BUNDLE_MAX_ITEMS))).toMatchObject({ ok: true });
    expect(checkBundle(many(BUNDLE_MAX_ITEMS + 1))).toMatchObject({ ok: false, reason: expect.stringContaining("51 facts") });
  });

  test("cuts a long title to 300 characters on a character boundary", () => {
    const short = bundleOf("  Ship\nthe   cache ", [item("x")]);
    expect(short.title).toBe("Ship the cache");
    const long = bundleOf("é😀".repeat(200), [item("x")]);
    expect([...long.title]).toHaveLength(300);
    expect(long.title.endsWith("…")).toBe(true);
    expect(long.title).not.toContain("\uFFFD");
  });
});

describe("excerpts", () => {
  test("are cut on a character boundary, never inside a multi-byte character", () => {
    const text = "ab😀cd😀ef";
    for (let max = 3; max <= utf8Bytes(text); max += 1) {
      const cut = trimToBytes(text, max);
      expect(utf8Bytes(cut)).toBeLessThanOrEqual(max);
      expect(cut).not.toContain("�");
      expect([...cut].every((char) => char !== "\uD83D")).toBe(true);
    }
    expect(trimToBytes(text, 100)).toBe(text);
  });

  test("become plain text and keep their status when trimmed", () => {
    expect(plainText("one\r\ntwo\u0007\n\n\n\nthree  ")).toBe("one\ntwo\n\nthree");
    const long = evidence({ kind: "run", id: "run_1" }, "verified", "x".repeat(5_000));
    expect(utf8Bytes(long.excerpt)).toBeLessThanOrEqual(2_048);
    expect(long.excerpt.endsWith("…")).toBe(true);
    expect(long.status).toBe("verified");
  });
});

describe("the understand link", () => {
  test("rides on #/learn and round-trips through the hash", () => {
    const href = understandHref({ kind: "pull_request", conn: "archon:local", id: "github.com/acme/forge#212" });
    expect(href.startsWith("#/learn?understand=pull_request&")).toBe(true);
    expect(parseUnderstand(href)).toEqual({ kind: "pull_request", conn: "archon:local", id: "github.com/acme/forge#212" });
  });

  test("ignores other queries and unknown kinds", () => {
    expect(parseUnderstand("#/learn")).toBeNull();
    expect(parseUnderstand("#/learn?panel=settings")).toBeNull();
    expect(parseUnderstand("#/learn?understand=planet&conn=a&id=b")).toBeNull();
    expect(parseUnderstand("#/learn?understand=item&conn=&id=qi_x")).toBeNull();
  });

  test("a repository catch-up carries its first day, and only a real one", () => {
    const request = { kind: "repository", conn: "archon:local", id: "repo_forge", since: "2026-11-01" } as const;
    expect(parseUnderstand(understandHref(request))).toEqual(request);
    expect(parseUnderstand("#/learn?understand=repository&conn=a&id=repo_x")).toBeNull();
    expect(parseUnderstand("#/learn?understand=repository&conn=a&id=repo_x&since=2026-02-31")).toBeNull();
    expect(() => understandHref({ kind: "repository", conn: "a", id: "repo_x" })).toThrow("only a repository catch-up has a first day");
    expect(() => understandHref({ kind: "item", conn: "a", id: "qi_x", since: "2026-11-01" })).toThrow("only a repository catch-up has a first day");
  });
});
