import { afterEach, describe, expect, test, vi } from "vitest";
import { acceptPairLink, forgetPairCode, offeredPairCode, PAIR_ROUTE, readPairLink } from "./pair-link.ts";

afterEach(() => forgetPairCode());

describe("the pairing URL", () => {
  test("reads the code aetherd puts in the fragment of /pair", () => {
    expect(readPairLink({ pathname: "/pair", hash: "#code=ABCD-1234" })).toBe("ABCD-1234");
    expect(readPairLink({ pathname: "/pair/", hash: "#code=%20abcd-1234%20" })).toBe("abcd-1234");
  });

  test("ignores every other page, and a pairing URL without a code", () => {
    for (const [pathname, hash] of [["/", "#code=ABCD-1234"], ["/pairing", "#code=X"], ["/pair", "#/pair/aether"], ["/pair", "#code="], ["/pair", ""]] as const) {
      expect(readPairLink({ pathname, hash }), `${pathname}${hash}`).toBeNull();
    }
  });

  test("opens the pairing route with the code offered once, and keeps it out of the address bar", () => {
    const history = { replaceState: vi.fn() };
    expect(acceptPairLink({ pathname: "/pair", hash: "#code=ABCD-1234" }, history)).toBe(true);
    expect(history.replaceState).toHaveBeenCalledExactlyOnceWith(null, "", `./${PAIR_ROUTE}`);
    expect(offeredPairCode("aether")).toBe("ABCD-1234");
    expect(offeredPairCode("archon"), "aetherd's code is no Archon code").toBe("");
    forgetPairCode();
    expect(offeredPairCode("aether")).toBe("");
  });

  test("leaves any other page as it is", () => {
    const history = { replaceState: vi.fn() };
    expect(acceptPairLink({ pathname: "/", hash: "#/fleet" }, history)).toBe(false);
    expect(history.replaceState).not.toHaveBeenCalled();
    expect(offeredPairCode("aether")).toBe("");
  });
});
