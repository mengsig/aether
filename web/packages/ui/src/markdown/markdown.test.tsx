// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, test } from "vitest";
import { Markdown } from "./markdown.tsx";
import { safeHref } from "./url.ts";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const roots: Root[] = [];
afterEach(async () => {
  await act(async () => { for (const root of roots.splice(0)) root.unmount(); });
  document.body.replaceChildren();
});

async function render(text: string): Promise<HTMLElement> {
  const container = document.createElement("div");
  document.body.append(container);
  const mounted = createRoot(container);
  roots.push(mounted);
  await act(async () => mounted.render(<Markdown text={text} />));
  const root = container.firstElementChild as HTMLElement | null;
  if (!root) throw new Error("Markdown rendered nothing");
  return root;
}

const html = async (text: string) => (await render(text)).innerHTML;

describe("safeHref", () => {
  test.each([
    ["https://example.com/a?b=1&c=2", "https://example.com/a?b=1&c=2"],
    ["HTTP://example.com", "HTTP://example.com"],
    ["mailto:me@example.com", "mailto:me@example.com"],
  ])("allows %s", (url, expected) => expect(safeHref(url)).toBe(expected));

  test.each([
    "javascript:alert(1)",
    " JaVa\tScript:alert(1)",
    "java&#x09;script:alert(1)",
    "java&#9;script:alert(1)",
    "java&Tab;script:alert(1)",
    "javascript&colon;alert(1)",
    "\u0001javascript:alert(1)",
    "java\nscript:alert(1)",
    "data:text/html;base64,PHNjcmlwdD4=",
    "vbscript:msgbox(1)",
    "file:///etc/passwd",
    "//evil.example/x",
    "/\\evil.example/x",
    "\\\\evil.example",
    "relative/path", "#section", "/docs/page",
    "",
    `https://example.com/${"a".repeat(3000)}`,
  ])("rejects %j", (url) => expect(safeHref(url)).toBeNull());
});

describe("hostile input", () => {
  const hostile = [
    "<script>alert(1)</script>",
    "<img src=x onerror=\"alert(1)\">",
    "<div onclick=\"x()\">hi</div>",
    "<iframe src=\"https://evil\"></iframe>",
    "[click](javascript:alert(1))",
    "[c2](JaVaScRiPt:alert(1))",
    "[c3]( javascript:alert(1))",
    "[d](data:text/html;base64,PHNjcmlwdD4=)",
    "<a href=\"javascript:alert(1)\">x</a>",
    "<svg onload=alert(1)>",
    "<style>body{display:none}</style>",
  ];

  test("never becomes an element, handler or unsafe link", async () => {
    const root = await render(hostile.join("\n\n"));
    expect(root.querySelectorAll("script, iframe, img, svg, style, object, embed")).toHaveLength(0);
    for (const element of root.querySelectorAll("*")) {
      expect([...element.attributes].filter((attribute) => attribute.name.startsWith("on"))).toEqual([]);
    }
    for (const anchor of root.querySelectorAll("a[href]")) {
      expect(anchor.getAttribute("href")).toMatch(/^(https?:|mailto:|\/(?!\/)|#)/i);
    }
    const text = root.textContent ?? "";
    for (const line of ["<script>alert(1)</script>", "<img src=x onerror=\"alert(1)\">", "<iframe src=\"https://evil\"></iframe>", "<svg onload=alert(1)>", "<style>body{display:none}</style>"]) {
      expect(text).toContain(line);
    }
    expect(text).toContain("click");
    expect(text).toContain("<a href=\"javascript:alert(1)\">x</a>");
  });

  test("keeps raw HTML in code as text too", async () => {
    const root = await render("```html\n<script>alert(1)</script>\n```\n\n`<img onerror=x>`");
    expect(root.querySelector("script, img")).toBeNull();
    expect(root.textContent).toContain("<script>alert(1)</script>");
    expect(root.textContent).toContain("<img onerror=x>");
  });

  test("an unsafe link renders its label as plain text", async () => {
    const root = await render("[click](javascript:alert(1)) after");
    expect(root.querySelector("a")).toBeNull();
    expect(root.querySelector("p")?.textContent).toBe("click after");
  });
});

describe("blocks", () => {
  test("splits paragraphs on blank lines and turns single newlines into breaks", async () => {
    const root = await render("one\ntwo\n\nthree");
    expect(root.className).toBe("md");
    expect(root.querySelectorAll("p")).toHaveLength(2);
    expect(root.querySelector("p")?.innerHTML).toBe("one<br>two");
  });

  test.each([["#", "h3"], ["##", "h4"], ["###", "h5"], ["####", "h6"], ["######", "h6"]])("heading %s renders %s", async (marks, tag) => {
    const root = await render(`${marks} Title ##`);
    expect(root.querySelector(tag)?.textContent).toBe("Title");
  });

  test("does not treat #tag or 7 hashes as a heading", async () => {
    const root = await render("#tag\n\n####### seven");
    expect(root.querySelector("h3, h4, h5, h6")).toBeNull();
    expect(root.querySelectorAll("p")).toHaveLength(2);
  });

  test("renders a fenced code block with its language and keeps the text verbatim", async () => {
    const root = await render("before\n```ts\nconst a = 1;\n\n  **not bold**\n```\nafter");
    const code = root.querySelector("pre > code");
    expect(code?.getAttribute("data-lang")).toBe("ts");
    expect(code?.textContent).toBe("const a = 1;\n\n  **not bold**");
    expect(root.querySelector("strong")).toBeNull();
    expect(root.querySelectorAll("p")).toHaveLength(2);
  });

  test("an unclosed fence runs to the end and an odd language is dropped", async () => {
    const root = await render("```<b>\nx\ny");
    const code = root.querySelector("pre > code");
    expect(code?.hasAttribute("data-lang")).toBe(false);
    expect(code?.textContent).toBe("x\ny");
  });

  test("renders bullet and numbered lists with one level of nesting", async () => {
    const root = await render("- a\n- b\n  - b1\n  - b2\n* c\n\n3. x\n4. y");
    const lists = [...root.children].filter((node) => node.tagName === "UL" || node.tagName === "OL");
    expect(lists.map((node) => node.tagName)).toEqual(["UL", "OL"]);
    expect([...(lists[0]?.children ?? [])].map((li) => li.firstChild?.textContent)).toEqual(["a", "b", "c"]);
    expect(root.querySelectorAll("ul ul > li")).toHaveLength(2);
    expect(lists[1]?.getAttribute("start")).toBe("3");
  });

  test("a list item continues onto an indented line", async () => {
    const root = await render("1. first\n   more\n2. second");
    expect(root.querySelectorAll("ol > li")).toHaveLength(2);
    expect(root.querySelector("li")?.textContent).toBe("firstmore");
  });

  test("a list can interrupt a paragraph but a year cannot", async () => {
    const root = await render("intro\n- a\n\nin\n2020. done");
    expect(root.querySelectorAll("ul > li")).toHaveLength(1);
    expect(root.querySelectorAll("ol")).toHaveLength(0);
  });

  test("renders blockquotes, including a nested one", async () => {
    const root = await render("> quoted **bold**\n> > deeper\n\nplain");
    expect(root.querySelector("blockquote strong")?.textContent).toBe("bold");
    expect(root.querySelector("blockquote blockquote")?.textContent).toBe("deeper");
    expect(root.lastElementChild?.tagName).toBe("P");
  });

  test("renders a horizontal rule", async () => {
    const root = await render("above\n\n---\n\nbelow");
    expect(root.querySelectorAll("hr")).toHaveLength(1);
    expect(root.querySelectorAll("p")).toHaveLength(2);
  });

  test("caps quote nesting instead of recursing without bound", async () => {
    const root = await render(`${">".repeat(50_000)} deep`);
    expect(root.querySelectorAll("blockquote").length).toBeLessThanOrEqual(5);
    expect(root.textContent).toContain("deep");
  });
});

describe("inline", () => {
  test("renders code, bold, italic, underscore italic and strike", async () => {
    expect(await html("`a<b` **b** *i* _u_ ~~s~~")).toBe(
      "<p><code class=\"md-code\">a&lt;b</code> <strong>b</strong> <em>i</em> <em>u</em> <del>s</del></p>",
    );
  });

  test("nests emphasis and keeps snake_case and lone markers literal", async () => {
    const root = await render("***both*** **a *b* c** snake_case_name 2 * 3 * 4 __init__");
    expect(root.querySelector("strong > em")?.textContent).toBe("both");
    expect(root.querySelectorAll("strong")).toHaveLength(2);
    expect(root.textContent).toContain("snake_case_name 2 * 3 * 4 __init__");
  });

  test("renders a link with a safe target and rel", async () => {
    const anchor = (await render("[docs](https://example.com/a_(b)) and [here](#top)")).querySelectorAll("a");
    expect(anchor[0]?.getAttribute("href")).toBe("https://example.com/a_(b)");
    expect(anchor[0]?.getAttribute("target")).toBe("_blank");
    expect(anchor[0]?.getAttribute("rel")).toBe("noopener noreferrer nofollow");
    expect(anchor).toHaveLength(1);
  });

  test("autolinks bare URLs without their trailing punctuation", async () => {
    const root = await render("see https://example.com/x, (http://a.example/b) and http://c.example/d_(e).");
    const hrefs = [...root.querySelectorAll("a")].map((a) => a.getAttribute("href"));
    expect(hrefs).toEqual(["https://example.com/x", "http://a.example/b", "http://c.example/d_(e)"]);
    expect(root.textContent).toBe("see https://example.com/x, (http://a.example/b) and http://c.example/d_(e).");
  });

  test("does not nest a link inside a link label", async () => {
    const root = await render("[see https://inner.example](https://outer.example)");
    expect(root.querySelectorAll("a")).toHaveLength(1);
    expect(root.querySelector("a")?.getAttribute("href")).toBe("https://outer.example");
  });

  test("a backslash escapes punctuation", async () => {
    expect((await render("\\*not italic\\* \\[x\\](https://a.example)")).textContent).toBe("*not italic* [x](https://a.example)");
  });

  test("leaves unbalanced markers as text", async () => {
    const root = await render("`open **open *open ~~open [open](x");
    expect(root.querySelector("code, strong, em, del, a")).toBeNull();
    expect(root.textContent).toBe("`open **open *open ~~open [open](x");
  });
});

describe("hostile size", () => {
  const timed = async (text: string) => {
    const start = performance.now();
    const root = await render(text);
    return { root, ms: performance.now() - start };
  };

  const prose = "The quick brown fox jumps over the lazy dog while the build runs on every branch and the tests stay green, so nobody **worries** about the `release` until the merge lands.\nA second line follows with more words, see https://example.com/docs for details.\n\n";

  // A loose bound: the suite runs files in parallel, so this guards against blow-ups, not jitter.
  test("renders a 200 KB message in well under 500 ms", () => {
    renderToStaticMarkup(<Markdown text={prose.repeat(100)} />);
    const text = prose.repeat(Math.ceil(200_000 / prose.length));
    const start = performance.now();
    const markup = renderToStaticMarkup(<Markdown text={text} />);
    const ms = performance.now() - start;
    expect(markup.match(/<a /g)?.length).toBeGreaterThan(500);
    expect(ms).toBeLessThan(500);
  });

  test("renders a 200 KB message dense with every construct", async () => {
    const dense = "Some **bold** text with `code`, a [link](https://example.com/x) and https://auto.example/y.\nsecond line\n\n- item *one*\n- item two\n\n> quote\n\n";
    const { root, ms } = await timed(dense.repeat(Math.ceil(200_000 / dense.length)));
    expect(root.querySelectorAll("a").length).toBeGreaterThan(1000);
    expect(ms).toBeLessThan(2000);
  });

  test.each([
    ["unclosed brackets", "[".repeat(200_000)],
    ["unclosed backticks", "`a ``b ```c ".repeat(16_000)],
    ["unclosed stars", "*a ".repeat(66_000)],
    ["unclosed links", "[a](((".repeat(33_000)],
    ["repeated autolinks", "http://".repeat(28_000)],
    ["nested emphasis", "*a ".repeat(50_000) + "a* ".repeat(50_000)],
    ["deep list indent", `${" ".repeat(1000)}- x\n`.repeat(190)],
  ])("stays fast on %s", async (_name, text) => {
    const { root, ms } = await timed(text);
    expect(root.textContent?.length).toBeGreaterThan(0);
    expect(ms).toBeLessThan(2000);
  });
});
