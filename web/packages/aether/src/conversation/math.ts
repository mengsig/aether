// A readable stand-in for TeX: common commands become their Unicode symbols, digits and simple
// letters after ^ and _ become super- and subscripts, \frac{a}{b} becomes a⁄b. It is a fallback
// presentation, not a typesetter; the source stays available as the span's label.

import { invariant } from "../fleet/invariant.ts";

const MAX_TEX = 4_000;

const SYMBOLS: Readonly<Record<string, string>> = {
  alpha: "α", beta: "β", gamma: "γ", delta: "δ", epsilon: "ε", varepsilon: "ε", zeta: "ζ", eta: "η", theta: "θ",
  iota: "ι", kappa: "κ", lambda: "λ", mu: "μ", nu: "ν", xi: "ξ", pi: "π", rho: "ρ", sigma: "σ", tau: "τ",
  upsilon: "υ", phi: "φ", varphi: "φ", chi: "χ", psi: "ψ", omega: "ω",
  Gamma: "Γ", Delta: "Δ", Theta: "Θ", Lambda: "Λ", Xi: "Ξ", Pi: "Π", Sigma: "Σ", Phi: "Φ", Psi: "Ψ", Omega: "Ω",
  cdot: "·", times: "×", div: "÷", pm: "±", mp: "∓", le: "≤", leq: "≤", ge: "≥", geq: "≥", neq: "≠", ne: "≠",
  approx: "≈", equiv: "≡", sim: "∼", propto: "∝", infty: "∞", partial: "∂", nabla: "∇", sum: "∑", prod: "∏",
  int: "∫", oint: "∮", sqrt: "√", in: "∈", notin: "∉", subset: "⊂", subseteq: "⊆", supset: "⊃", cup: "∪",
  cap: "∩", emptyset: "∅", forall: "∀", exists: "∃", neg: "¬", land: "∧", lor: "∨", to: "→", rightarrow: "→",
  leftarrow: "←", Rightarrow: "⇒", Leftarrow: "⇐", leftrightarrow: "↔", iff: "⇔", mapsto: "↦", ldots: "…",
  cdots: "⋯", dots: "…", circ: "∘", deg: "°", prime: "′", langle: "⟨", rangle: "⟩", mid: "∣", ell: "ℓ",
  quad: " ", qquad: "  ",
};

const SUPERSCRIPTS: Readonly<Record<string, string>> = {
  0: "⁰", 1: "¹", 2: "²", 3: "³", 4: "⁴", 5: "⁵", 6: "⁶", 7: "⁷", 8: "⁸", 9: "⁹", "+": "⁺", "-": "⁻", "=": "⁼",
  "(": "⁽", ")": "⁾", n: "ⁿ", i: "ⁱ", T: "ᵀ",
};
const SUBSCRIPTS: Readonly<Record<string, string>> = {
  0: "₀", 1: "₁", 2: "₂", 3: "₃", 4: "₄", 5: "₅", 6: "₆", 7: "₇", 8: "₈", 9: "₉", "+": "₊", "-": "₋", "=": "₌",
  "(": "₍", ")": "₎", i: "ᵢ", j: "ⱼ", k: "ₖ", n: "ₙ", m: "ₘ", x: "ₓ",
};

/** The script form of `text` when every character has one, else the text marked with ^ or _. */
function script(text: string, table: Readonly<Record<string, string>>, mark: string): string {
  let out = "";
  for (const ch of text) {
    const mapped = table[ch];
    if (mapped === undefined) return text.length === 1 ? `${mark}${text}` : `${mark}(${text})`;
    out += mapped;
  }
  return out;
}

export function prettyTex(tex: string): string {
  invariant(typeof tex === "string", "TeX arrives as text");
  if (tex.length > MAX_TEX) return tex;
  const out = tex
    .replace(/\\(?:left|right|displaystyle)\b/g, "")
    .replace(/\\(?:text|mathrm|mathbf|mathit|operatorname)\{([^{}]*)\}/g, "$1")
    .replace(/\\frac\{([^{}]*)\}\{([^{}]*)\}/g, "$1⁄$2")
    .replace(/\\sqrt\{([^{}]*)\}/g, "√($1)")
    .replace(/\\([A-Za-z]+)/g, (whole, name: string) => SYMBOLS[name] ?? whole)
    .replace(/\\[,;: !]/g, " ")
    .replace(/\^\{([^{}]*)\}|\^(\S)/g, (_whole, group: string | undefined, single: string | undefined) => script(group ?? single ?? "", SUPERSCRIPTS, "^"))
    .replace(/_\{([^{}]*)\}|_(\S)/g, (_whole, group: string | undefined, single: string | undefined) => script(group ?? single ?? "", SUBSCRIPTS, "_"))
    .replace(/[{}]/g, "");
  invariant(out.length <= tex.length * 2, "the readable form stays close to the source's length");
  return out;
}
