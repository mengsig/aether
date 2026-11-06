export const MAX_DIAGRAM_LENGTH = 20_000;

/** Configuration, interactions, HTML and external resources stay out of the rendering DOM. */
export function diagramRefusal(source: string): string | null {
  if (source.trim() === "") return "The diagram is empty.";
  if (source.length > MAX_DIAGRAM_LENGTH) return "The diagram exceeds the rendering limit.";
  if (/%%\s*\{|^\s*---/.test(source)) return "Diagram configuration is disabled.";
  if (/<\/?[a-z!]|&(?:#(?:x[\da-f]+|\d+)|lt|gt);/i.test(source)) return "HTML labels are disabled.";
  if (/(?:^|[\n;])\s*click\b|\b(?:img|image|icon)\s*:/i.test(source)) return "Diagram interactions and images are disabled.";
  if (/(?:https?|data|javascript|file):|url\s*\(|!\[|\/\//i.test(source)) return "External diagram resources are disabled.";
  if (/(?:^|[\n;])\s*(?:style|classDef|linkStyle)\b/i.test(source)) return "Diagram styling uses the app theme.";
  return null;
}
