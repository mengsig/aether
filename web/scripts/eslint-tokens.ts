// Components take colours and durations only from the CONTRACTS.md §4 tokens. This rule rejects raw
// ones in source strings, in style objects and in every stylesheet a source imports.

import fs from "node:fs";
import path from "node:path";
import type { Rule } from "eslint";

const HEX_COLOUR = /(?<![\w&-])#(?:[0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})(?![\w-])/i;
const COLOUR_FUNCTION = /(?<![\w-])(?:rgba?|hsla?|hwb|lab|lch|oklab|oklch|color)\(/i;
const DURATION = /(?<![\w.#-])(?:\d+\.?\d*|\.\d+)m?s(?![\w-])/i;
const ZERO_DURATION = /^(?:0+\.?0*|\.0+)m?s$/i;
// The CSS named colours; `transparent` and `currentColor` carry no colour of their own.
const NAMED_COLOURS = new Set(`aliceblue antiquewhite aqua aquamarine azure beige bisque black blanchedalmond blue
  blueviolet brown burlywood cadetblue chartreuse chocolate coral cornflowerblue cornsilk crimson cyan darkblue
  darkcyan darkgoldenrod darkgray darkgreen darkgrey darkkhaki darkmagenta darkolivegreen darkorange darkorchid
  darkred darksalmon darkseagreen darkslateblue darkslategray darkslategrey darkturquoise darkviolet deeppink
  deepskyblue dimgray dimgrey dodgerblue firebrick floralwhite forestgreen fuchsia gainsboro ghostwhite gold
  goldenrod gray green greenyellow grey honeydew hotpink indianred indigo ivory khaki lavender lavenderblush
  lawngreen lemonchiffon lightblue lightcoral lightcyan lightgoldenrodyellow lightgray lightgreen lightgrey
  lightpink lightsalmon lightseagreen lightskyblue lightslategray lightslategrey lightsteelblue lightyellow lime
  limegreen linen magenta maroon mediumaquamarine mediumblue mediumorchid mediumpurple mediumseagreen
  mediumslateblue mediumspringgreen mediumturquoise mediumvioletred midnightblue mintcream mistyrose moccasin
  navajowhite navy oldlace olive olivedrab orange orangered orchid palegoldenrod palegreen paleturquoise
  palevioletred papayawhip peru pink plum powderblue purple rebeccapurple red rosybrown royalblue saddlebrown
  salmon sandybrown seagreen seashell sienna silver skyblue slateblue slategray slategrey snow springgreen
  steelblue tan teal thistle tomato turquoise violet wheat white whitesmoke yellow yellowgreen`.split(/\s+/));
// Style keys whose numbers are durations, as in React styles and Element.animate() options.
const TIMING_KEY = /^(?:duration|delay|endDelay|transition\w*|animation\w*)$/i;
const TOKEN_SHEET = path.join("packages", "ui", "src", "tokens.css");
const COLOUR_ATTRIBUTES = new Set(["fill", "stroke", "color", "stopColor", "stop-color", "floodColor", "flood-color"]);

const COLOUR_HINT = "use a --color-* token from tokens.css (CONTRACTS.md §4)";
const DURATION_HINT = "use a --motion-* token from tokens.css, so reduced motion can zero it (CONTRACTS.md §4)";

interface RawValue {
  readonly message: string;
  /** Where the raw value starts in the text. */
  readonly index: number;
}

/** The first raw colour or duration in a CSS value or source string, or undefined. */
export function rawValueProblem(text: string, options: { colours: boolean; durations: boolean; named: boolean }): RawValue | undefined {
  const colour = options.colours ? HEX_COLOUR.exec(text) ?? COLOUR_FUNCTION.exec(text) : null;
  if (colour !== null) return { message: `raw colour ${JSON.stringify(colour[0])}: ${COLOUR_HINT}`, index: colour.index };
  if (options.named) {
    const named = [...text.matchAll(/[a-z]+/gi)].find((word) => NAMED_COLOURS.has(word[0].toLowerCase()));
    if (named !== undefined) return { message: `raw colour ${JSON.stringify(named[0])}: ${COLOUR_HINT}`, index: named.index };
  }
  if (!options.durations) return undefined;
  const raw = [...text.matchAll(new RegExp(DURATION.source, "gi"))].find((match) => !ZERO_DURATION.test(match[0]));
  return raw === undefined ? undefined : { message: `raw duration ${JSON.stringify(raw[0])}: ${DURATION_HINT}`, index: raw.index };
}

/** Each raw colour or duration in a stylesheet's declarations, as `line: message`. */
export function stylesheetProblems(css: string): string[] {
  const text = css.replace(/\/\*[\s\S]*?\*\//g, (comment) => comment.replace(/[^\n]/g, " "));
  const lineOf = (index: number) => text.slice(0, index).split("\n").length;
  const problems: string[] = [];
  for (const rule of text.matchAll(/@import\b/g)) {
    problems.push(`${lineOf(rule.index)}: @import hides a sheet from this check; import each sheet from a source module`);
  }
  // Only declaration values, which end at `;` or `}`: a selector such as `#id` or `:is(.red)` ends at `{`.
  for (const declaration of text.matchAll(/(?:^|[;{])\s*(?:--[\w-]+|[a-z-]+)\s*:([^;{}]*)(?=[;}])/gim)) {
    const value = declaration[1] ?? "";
    const raw = rawValueProblem(value, { colours: true, durations: true, named: true });
    const valueStart = declaration.index + declaration[0].length - value.length;
    if (raw !== undefined) problems.push(`${lineOf(valueStart + raw.index)}: ${raw.message}`);
  }
  return problems.sort((a, b) => Number.parseInt(a, 10) - Number.parseInt(b, 10));
}

function keyName(property: { key?: { type?: string; name?: unknown; value?: unknown }; computed?: boolean }): string | undefined {
  if (property.computed === true) return undefined;
  const name = property.key?.type === "Identifier" ? property.key.name : property.key?.value;
  return typeof name === "string" ? name : undefined;
}

/** The name of the JSX attribute whose value holds this node, without crossing a function, or undefined. */
function enclosingAttribute(node: Rule.Node): string | undefined {
  for (let current: Rule.Node | null = node.parent; current !== null; current = current.parent) {
    const attribute = current as unknown as { type: string; name?: { name?: unknown } };
    if (attribute.type === "JSXAttribute") return typeof attribute.name?.name === "string" ? attribute.name.name : undefined;
    if (current.type === "Program" || current.type.startsWith("Function") || current.type === "ArrowFunctionExpression") return undefined;
  }
  return undefined;
}

function stringValue(node: unknown): string | undefined {
  const literal = node as { type?: string; value?: unknown; quasis?: Array<{ value: { cooked?: unknown } }> };
  if (literal.type === "Literal") return typeof literal.value === "string" ? literal.value : undefined;
  if (literal.type !== "TemplateLiteral") return undefined;
  return (literal.quasis ?? []).map((quasi) => (typeof quasi.value.cooked === "string" ? quasi.value.cooked : "")).join(" ");
}

function stylesheetReport(sourceFile: string, specifier: string): string[] {
  const sheet = path.resolve(path.dirname(sourceFile), specifier.replace(/\?.*$/, ""));
  if (sheet.endsWith(path.sep + TOKEN_SHEET)) return [];
  let css: string;
  try {
    css = fs.readFileSync(sheet, "utf8");
  } catch (error) {
    return [`cannot read ${specifier} to check it for raw colours and durations: ${String(error)}`];
  }
  return stylesheetProblems(css).map((problem) => `${specifier}:${problem}`);
}

const rule: Rule.RuleModule = {
  meta: {
    type: "problem",
    docs: { description: "Take colours and durations from the CONTRACTS.md §4 tokens, never raw values" },
    schema: [],
  },
  create(context) {
    const checkString = (node: Rule.Node, text: string | undefined) => {
      if (text === undefined) return;
      const attribute = enclosingAttribute(node);
      const style = attribute === "style";
      const named = style || (attribute !== undefined && COLOUR_ATTRIBUTES.has(attribute));
      const raw = rawValueProblem(text, { colours: true, durations: style, named });
      if (raw !== undefined) context.report({ node, message: raw.message });
    };
    // A timing key outside a style attribute, as in `{ duration: 200 }` or `el.style.transition = "1s"`.
    const checkTiming = (node: Rule.Node, key: string | undefined, value: { type?: string; value?: unknown }) => {
      if (key === undefined || !TIMING_KEY.test(key) || value.type !== "Literal") return;
      if (typeof value.value === "number" && value.value !== 0) {
        context.report({ node, message: `raw duration ${value.value} for ${key}: ${DURATION_HINT}` });
      } else if (typeof value.value === "string" && enclosingAttribute(node) !== "style") {
        const raw = rawValueProblem(value.value, { colours: false, durations: true, named: false });
        if (raw !== undefined) context.report({ node, message: raw.message });
      }
    };
    return {
      Literal: (node) => checkString(node, stringValue(node)),
      TemplateLiteral: (node) => checkString(node, stringValue(node)),
      Property: (node) => checkTiming(node, keyName(node as unknown as Parameters<typeof keyName>[0]), node.value),
      AssignmentExpression: (node) => {
        const target = node.left as { type: string; computed?: boolean; property?: { type?: string; name?: unknown } };
        const key = target.type === "MemberExpression" && target.computed !== true && typeof target.property?.name === "string"
          ? target.property.name : undefined;
        checkTiming(node, key, node.right);
      },
      ImportDeclaration: (node) => {
        const specifier = node.source.value;
        if (typeof specifier !== "string" || !specifier.startsWith(".") || !/\.css(?:\?|$)/.test(specifier)) return;
        for (const message of stylesheetReport(context.filename, specifier)) context.report({ node, message });
      },
    };
  },
};

export default { rules: { "design-tokens": rule } };
