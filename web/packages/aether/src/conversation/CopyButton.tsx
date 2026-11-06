// A small "Copy" button for code and tool output: it says "Copied" for a moment, or says the copy
// failed (the clipboard can be refused), and announces either politely.

import { useEffect, useState } from "react";
import { invariant } from "../fleet/invariant.ts";

const RESET_MS = 1_600;

type CopyState = "idle" | "copied" | "failed";

async function writeClipboard(text: string): Promise<void> {
  const clipboard = (globalThis.navigator as Navigator | undefined)?.clipboard;
  if (clipboard === undefined) throw new Error("this browser offers no clipboard");
  await clipboard.writeText(text);
}

export function CopyButton({ text, label = "Copy", what }: { readonly text: string; readonly label?: string; readonly what: string }) {
  invariant(what.trim() !== "", "a copy button says what it copies");
  const [state, setState] = useState<CopyState>("idle");
  useEffect(() => {
    if (state === "idle") return undefined;
    const timer = setTimeout(() => setState("idle"), RESET_MS);
    return () => clearTimeout(timer);
  }, [state]);
  const copy = () => {
    writeClipboard(text).then(() => setState("copied"), (error: unknown) => {
      setState("failed");
      console.warn(`Could not copy ${what}:`, error);
    });
  };
  const shown = state === "copied" ? "Copied" : state === "failed" ? "Copy failed" : label;
  return (
    <button type="button" className="conv-copy" data-state={state} aria-label={state === "idle" ? `Copy ${what}` : undefined} onClick={copy}>
      <span aria-live="polite">{shown}</span>
    </button>
  );
}
