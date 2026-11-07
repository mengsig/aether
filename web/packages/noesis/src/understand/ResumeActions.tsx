// What the sheet says and offers when its evidence already started a session: continue it, or start fresh.

import { navigate, routeHref } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { Button } from "@aether-ui/ui";
import { dateTime } from "../learn/model.ts";
import type { Resumable } from "./resume.ts";

export function ContinueButton({ session }: { readonly session: aether.NoesisSession }) {
  return <Button variant="primary" onClick={() => navigate(routeHref("#/learn/sessions/:id", { id: session.id }))}>Continue where you left off</Button>;
}

export function ResumeNote({ resume }: { readonly resume: Resumable }) {
  switch (resume.state) {
    case "found":
      return <p className="learn-muted" data-testid="understand-resume">You already started “{resume.session.goal}” from this evidence, last worked on {dateTime(resume.session.updated_at)}.</p>;
    case "unknown":
      return <p className="learn-muted" role="status">Could not check for an earlier session: {resume.reason}</p>;
    default:
      return null;
  }
}
