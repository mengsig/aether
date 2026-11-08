// The session's composer, pinned under the chat: Enter sends, Shift+Enter starts a new line, and what
// was typed is kept per session until that text is sent; text typed meanwhile stays.

import { useDraft } from "@aether-ui/core";
import { Button, TextArea } from "@aether-ui/ui";
import { useState, type FormEvent, type KeyboardEvent } from "react";
import { draftKey, invariant } from "./model.ts";

const TEXT_MAX = 20_000;
const EMPTY_REPLY: { readonly text: string } = { text: "" };

export interface ComposerProps {
  readonly sessionId: string;
  /** A question is open: it is answered in its card, not here. */
  readonly waiting: boolean;
  readonly problem: boolean;
  readonly disabled: boolean;
  /** Resolves to null when sent, else to why not. */
  readonly onSend: (text: string) => Promise<string | null>;
}

/** Whether a key press in the composer sends: Enter alone, never while an IME is composing. */
export function sendsOnKey(event: Pick<KeyboardEvent, "key" | "shiftKey" | "altKey"> & { readonly nativeEvent?: { readonly isComposing?: boolean } }): boolean {
  if (event.key !== "Enter" || event.shiftKey || event.altKey) return false;
  return event.nativeEvent?.isComposing !== true;
}

export function Composer({ sessionId, waiting, problem, disabled, onSend }: ComposerProps) {
  invariant(sessionId.length > 0, "a composer belongs to a session");
  const draft = useDraft(draftKey(sessionId, "reply"), EMPTY_REPLY);
  const [sending, setSending] = useState(false);
  const trimmed = draft.value.text.trim();
  const blocked = disabled || waiting;
  const send = async () => {
    if (trimmed === "" || blocked || sending) return;
    invariant(trimmed.length <= TEXT_MAX, "the composer keeps a message within its limit");
    setSending(true);
    const failure = await onSend(trimmed);
    setSending(false);
    if (failure === null) draft.clearIfUnchanged();
  };
  const submit = (event: FormEvent) => {
    event.preventDefault();
    void send();
  };
  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (!sendsOnKey(event)) return;
    event.preventDefault();
    void send();
  };
  const placeholder = waiting ? "Answer the question above to go on" : problem ? "Your answer, or ask for a hint" : "Ask, answer or say what is unclear";
  return (
    <form className="learn-composer" onSubmit={submit} aria-label="Reply">
      <TextArea label="Message your teacher" hideLabel data-testid="learn-reply" rows={2} maxLength={TEXT_MAX} disabled={blocked} placeholder={placeholder}
        value={draft.value.text} onChange={(event) => draft.update({ text: event.target.value })} onKeyDown={onKeyDown} />
      <div className="learn-composer-bar">
        <span className="learn-muted learn-composer-keys" aria-hidden="true">Enter to send · Shift+Enter for a new line</span>
        <Button variant="primary" type="submit" data-testid="learn-send" disabled={blocked || sending || trimmed === ""}>{sending ? "Sending…" : "Send"}</Button>
      </div>
    </form>
  );
}
