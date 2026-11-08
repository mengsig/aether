# Shared lesson and chat rendering

Import `Markdown` or `InlineMarkdown` from `@aether-ui/ui`; both take `{ text: string }`.
`Markdown` returns a block `<div>`; `InlineMarkdown` returns a phrasing-only `<span>` for
option labels, answer keys and summaries. Headings retain chat's h3–h6 hierarchy.

The parser builds React elements from fixed tags. Input HTML is text. Markdown images
become links or alt text, never remote images. Links allow only `http:`, `https:` and
`mailto:` and include `noopener noreferrer nofollow`. Fenced code uses a small lexical
highlighter whose tokens are React spans; it never produces HTML strings.

KaTeX loads only when a math expression mounts, emits native MathML, and uses
`throwOnError:false`, `trust:false`, `strict:"error"`, `maxExpand:200` and `maxSize:20`.
Only its escaped, untrusted-mode output reaches `dangerouslySetInnerHTML`. Expressions
are capped at 8,192 characters, and macros are never shared. Dollar delimiters require
non-whitespace on their inside and a closing dollar not followed by a digit; escaped
dollars and code stay literal. `$$` display math occupies its own line or pair of lines.
Native MathML avoids vendor stylesheets and font requests in Chromium/Electron and browsers.

Mermaid loads only when an allowed fence mounts. It uses `securityLevel:"strict"`,
`htmlLabels:false`, a 20,000-character/200-edge limit, no binding of interactions, and
serialized rendering so global configuration cannot race across themes. Configuration,
HTML labels, click/callback commands, external resources and custom styles are refused
before parsing. App token colors supply its theme. Mermaid measures in a same-origin sandboxed
frame with its own restricted CSP; only that frame permits inline styles. Its script can reach the
parent, so source checks and Mermaid's strict sanitization remain essential. The frame returns a sanitized SVG
as a local `data:image/svg+xml` image:
[SVG image documents](https://developer.mozilla.org/en-US/docs/Web/SVG/Guides/SVG_as_an_image)
disable scripts and external resources, with no SVG inserted in
the application DOM. The main page keeps its strict script and style policies. Source
remains available under every successful image; refusals, parse errors and lazy-load
failures visibly retain source instead. This intentionally excludes interactive diagrams.

Tests: `markdown.test.tsx` preserves the chat parser coverage; `rich.test.tsx` covers
new elements, dollar ambiguity and KaTeX XSS; `mermaid.test.tsx` covers the library boundary,
strict settings and source fallback. `lesson.perf.test.tsx` renders 200 messages exceeding
200 KB, including 400 math expressions, within a generous five-second bound. The original
large/adversarial parser checks remain in place. Browser verification covers actual SVG
layout and CSP, which happy-dom cannot model.

The dependencies intentionally use maintained Mermaid 11: Mermaid 12 adds an ELK runtime
dependency under EPL/GPL, outside this repository's licence allowlist. A scoped npm override
shares the patched KaTeX 0.19 engine with Mermaid; its declared 0.16 dependency has the
trust-bypass advisory GHSA-238p-pmpm-9mq7. The browser fixture also exercises Mermaid math
labels to check this API compatibility. Direct
MIT notices ship in `apps/web/public/THIRD-PARTY-NOTICES-rendering.txt`; `npm run licenses`
checks every locked dependency. Two of Mermaid's own dependencies need named runtime exceptions
in `web/scripts/licenses.ts`: khroma (an MIT licence file, though its manifest declares none) and
robust-predicates (the Unlicense, a public-domain dedication).
