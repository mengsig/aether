// An item's artifact store: the files, and each file's text read a page at a time in a sheet.

import { useCommand } from "@aether-ui/core";
import type { archon } from "@aether-ui/protocol";
import { Button, EmptyState, ErrorState, Sheet } from "@aether-ui/ui";
import { useState } from "react";
import { useRead, type ArchonAccess } from "../queue/archon.ts";
import { invariant, replyOf } from "../queue/model.ts";
import { bytes } from "./facts.ts";

export const PAGE_BYTES = 65_536;

interface Pages { readonly text: string; readonly next: number | null; readonly total: number }

function ArtifactSheet({ access, item, name, onClose }: { readonly access: ArchonAccess; readonly item: string; readonly name: string; readonly onClose: () => void }) {
  const send = useCommand(access.conn);
  const first = useRead<archon.ArtifactPage>(access, { cmd: "read_artifact", item, name, offset: 0, limit: PAGE_BYTES }, ["artifact_page"]);
  const [more, setMore] = useState<Pages | null>(null);
  const [error, setError] = useState<{ text: string; offset: number } | null>(null);
  const pages: Pages | null = more ?? (first.data ? { text: first.data.text, next: first.data.next_offset ?? null, total: first.data.total_bytes } : null);
  const loadMore = async (shown: Pages, offset: number) => {
    invariant(offset > 0 && shown.next === offset, "the next page starts where the shown text ends");
    try {
      const page = replyOf<archon.ArtifactPage>(await send({ cmd: "read_artifact", item, name, offset, limit: PAGE_BYTES }), "artifact_page");
      invariant(page.offset === offset, `the page starts where it was asked, ${offset}, not ${page.offset}`);
      setError(null);
      setMore({ text: shown.text + page.text, next: page.next_offset ?? null, total: page.total_bytes });
    } catch (failure) {
      setError({ text: failure instanceof Error ? failure.message : String(failure), offset });
    }
  };
  const failed = error ?? (first.error ? { text: first.error, offset: 0 } : null);
  return (
    <Sheet open onClose={onClose} title={name} description={pages ? bytes(pages.total) : "Loading"}>
      {pages ? <pre className="item-artifact" tabIndex={0} aria-label={`Contents of ${name}`}>{pages.text}</pre> : null}
      {failed ? <ErrorState title="Could not read the file" message={failed.text}
        onRetry={() => { if (failed.offset === 0) first.reload(); else if (pages) void loadMore(pages, failed.offset); }} /> : null}
      {pages && pages.next !== null ? (
        <Button onClick={() => void loadMore(pages, pages.next as number)}>Load more ({bytes(Math.max(0, pages.total - pages.next))} left)</Button>
      ) : pages ? <p className="item-muted">End of file</p> : null}
    </Sheet>
  );
}

export function Artifacts({ access, item, version }: { readonly access: ArchonAccess; readonly item: string; readonly version: unknown }) {
  const listing = useRead<archon.ArtifactListing>(access, { cmd: "list_artifacts", item }, ["artifacts"], version);
  const [open, setOpen] = useState<string | null>(null);
  if (listing.error && !listing.data) return <ErrorState title="Could not list artifacts" message={listing.error} onRetry={listing.reload} />;
  if (!listing.data) return <p className="item-muted">Loading artifacts…</p>;
  const files = listing.data.files;
  if (files.length === 0) return <EmptyState title="No artifacts">Files the stages publish show up here.</EmptyState>;
  return (
    <>
      <ul className="item-list">
        {files.map((file) => (
          <li key={file.name}>
            <button type="button" className="item-row-button" onClick={() => setOpen(file.name)}>
              <span>{file.name}</span><span className="item-muted">{bytes(file.bytes)}</span>
            </button>
          </li>
        ))}
      </ul>
      {listing.data.omitted > 0 ? <p className="item-muted">{listing.data.omitted} more not listed</p> : null}
      {open ? <ArtifactSheet access={access} item={item} name={open} onClose={() => setOpen(null)} /> : null}
    </>
  );
}
