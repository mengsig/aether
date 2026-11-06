// @vitest-environment happy-dom
import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, test } from "vitest";
import { InlineMarkdown, Markdown } from "./markdown.tsx";
import { renderMath } from "./math-render.ts";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const roots: Root[] = [];
afterEach(async () => {
  await act(async () => { for (const root of roots.splice(0)) root.unmount(); });
  document.body.replaceChildren();
});

async function render(node: ReactNode): Promise<HTMLElement> {
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  roots.push(root);
  await act(async () => { root.render(node); });
  // Importing once also exercises the same dynamic module used by the component.
  await act(async () => { await import("./math-render.ts"); });
  return container;
}

describe("shared rich markdown", () => {
  test("renders tables with aligned headers, inline cells and escaped pipes", async () => {
    const root = await render(<Markdown text={"intro\n\n| Name | Value |\n| :--- | ---: |\n| **mass** | `a|b` |\n| a\\|b | 2 |"} />);
    expect(root.querySelectorAll("th[scope=col]")).toHaveLength(2);
    expect(root.querySelectorAll("tbody tr")).toHaveLength(2);
    expect(root.querySelector("td strong")?.textContent).toBe("mass");
    expect(root.querySelector("td code")?.textContent).toBe("a|b");
    expect(root.querySelector("tbody tr:last-child td")?.textContent).toBe("a|b");
    expect(root.querySelectorAll('[data-align="right"]')).toHaveLength(3);
  });

  test("highlights code with token spans and preserves the exact source", async () => {
    const source = 'const message = "<img onerror=x>"; // note\nreturn 42;';
    const root = await render(<Markdown text={`\`\`\`js\n${source}\n\`\`\``} />);
    expect(root.querySelector("code.language-js")?.textContent).toBe(source);
    for (const kind of ["keyword", "string", "comment", "number"]) expect(root.querySelector(`.md-token-${kind}`)).not.toBeNull();
    expect(root.querySelector("img")).toBeNull();
  });

  test("renders inline and display math as native MathML", async () => {
    const root = await render(<Markdown text={"Energy $E=mc^2$.\n\n$$\n\\frac{a}{b}\n$$\n\n$$x^2$$"} />);
    expect(root.querySelectorAll("math")).toHaveLength(3);
    expect(root.querySelectorAll('math[display="block"]')).toHaveLength(2);
    expect(root.querySelector("msup")).not.toBeNull();
    expect(root.querySelector("mfrac")).not.toBeNull();
  });

  test.each(["Prices $5 and $10", "$ x$", "$x $", "$x$2", "\\$y\\$", "`$z$`"])("preserves non-math dollars in %s", async (text) => {
    const root = await render(<Markdown text={text} />);
    expect(root.querySelector("math")).toBeNull();
    expect(root.textContent).toContain(text.replaceAll("\\", "").replaceAll("`", ""));
  });

  test.each(["Price $5", "Prices $5 and $10", "Prices $5, $10 and $15"])("%s stays literal when a real expression follows", async (prices) => {
    const root = await render(<Markdown text={`${prices}; solve $x^2$.`} />);
    expect(root.querySelectorAll("math")).toHaveLength(1);
    expect(root.textContent).toContain(`${prices}; solve`);
  });

  test("inline labels have phrasing content only, with emphasis, code, links and math", async () => {
    const root = await render(<InlineMarkdown text={"# **bold**\n- *italic* `code` [docs](https://example.test) $x^2$\n$$block$$"} />);
    expect(root.querySelector("h1,h2,h3,p,div,ul,li,pre,table,figure")).toBeNull();
    expect(root.querySelector("strong")?.textContent).toBe("bold");
    expect(root.querySelector("em")?.textContent).toBe("italic");
    expect(root.querySelector("code")?.textContent).toBe("code");
    expect(root.querySelector("a")?.rel).toContain("noopener");
    expect(root.querySelectorAll("math")).toHaveLength(1);
    expect(root.textContent).toContain("$$block$$");
  });

  test("images become safe links or alt text without fetching images", async () => {
    const root = await render(<Markdown text={"![alt](https://example.test/remote.png) ![blocked](data:image/png;base64,x)"} />);
    expect(root.querySelector("img,iframe")).toBeNull();
    expect(root.querySelector("a")?.textContent).toBe("alt");
    expect(root.textContent).toContain("blocked");
  });

  test("malformed and oversized math stays readable", async () => {
    const root = await render(<Markdown text={"$\\frac{$\n\n$$unterminated\n\n" + "$" + "x".repeat(9000) + "$"} />);
    expect(root.querySelector(".katex-error")?.textContent).toBe("\\frac{");
    expect(root.textContent).toContain("$$unterminated");
    expect(root.textContent).toContain("x".repeat(9000));
  });
});

describe("KaTeX trust boundary", () => {
  test.each([
    String.raw`\href{javascript:alert(1)}{click}`, String.raw`\href{https://evil.test}{click}`,
    String.raw`\url{https://evil.test}`, String.raw`\htmlClass{owned}{x}`,
    String.raw`\htmlId{owned}{x}`, String.raw`\htmlStyle{background:url(https://evil.test)}{x}`,
    String.raw`\htmlData{test=owned}{x}`, String.raw`\includegraphics{https://evil.test/image.png}`,
    String.raw`\text{<img src=x onerror=alert(1)>}`, String.raw`\text{<script>alert(1)</script>}`,
  ])("disables trust commands and escapes %s", (source) => {
    const container = document.createElement("div");
    container.innerHTML = renderMath(source, false);
    expect(container.querySelector("a,img,script,iframe,style,#owned,.owned,[data-test]")).toBeNull();
    for (const node of container.querySelectorAll("*")) {
      expect([...node.attributes].some((attribute) => /^(?:on|href|src)/i.test(attribute.name))).toBe(false);
      expect(node.getAttribute("style") ?? "").not.toMatch(/url\s*\(|background/i);
    }
  });

  test("caps macro expansion and does not share macro definitions between messages", () => {
    expect(renderMath(String.raw`\def\loop{\loop}\loop`, false)).toContain("katex-error");
    renderMath(String.raw`\gdef\owned{x}`, false);
    const isolated = document.createElement("div");
    isolated.innerHTML = renderMath(String.raw`\owned`, false);
    expect(isolated.textContent).toContain("owned");
    expect(isolated.querySelector("math mi")?.textContent).not.toBe("x");
    expect(() => renderMath("x".repeat(8193), false)).toThrow("bounded");
  });
});
