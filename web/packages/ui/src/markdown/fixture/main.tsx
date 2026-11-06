import { createRoot } from "react-dom/client";
import { Markdown, InlineMarkdown, ThemeProvider, Button, useTheme } from "../../index.ts";
import { invariant } from "../../classes.ts";
import "./fixture.css";

const lesson = `# Conservation of energy

**Energy** links *mass* to motion: $E=mc^2$. Prices $5 and $10 remain prices.

$$
\\int_0^1 x^2\\,dx = \\frac{1}{3}
$$

| Quantity | Symbol | Unit |
| :--- | :---: | ---: |
| Energy | $E$ | J |
| Mass | $m$ | kg |

- Read the [reference](https://example.test).
- Try a worked example.

\`\`\`ts
const mass = 2; // measured in kilograms
return mass;
\`\`\`

\`\`\`mermaid
flowchart LR
  A["$$E=mc^2$$"] --> B[Practice]
  B --> C[Understand]
\`\`\`

\`\`\`mermaid
sequenceDiagram
  Student->>Teacher: Why?
  Teacher-->>Student: Try this example
\`\`\`

\`\`\`mermaid
not a valid diagram
\`\`\`

\`\`\`mermaid
flowchart LR
A[<img src=x onerror=alert(1)>]
click A callback
\`\`\`

<script>alert(1)</script>
![Remote images are links](https://example.test/image.png)

$\\href{javascript:alert(1)}{blocked}$
`;

function Fixture() {
  const theme = useTheme();
  return <main className="markdown-fixture">
    <Button onClick={() => theme?.setPreference(theme.resolved === "dark" ? "light" : "dark")}>Toggle theme</Button>
    <Markdown text={lesson} />
    <label><input type="radio" name="answer" /><InlineMarkdown text="**Answer:** $E=mc^2$" /></label>
  </main>;
}

const root = document.getElementById("root");
invariant(root !== null, "the markdown fixture has a mount point");
createRoot(root).render(<ThemeProvider initialPreference="light"><Fixture /></ThemeProvider>);
