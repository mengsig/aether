import type { ReactNode } from "react";
import { classes, invariant } from "./classes.ts";

export interface PageHeaderProps {
  /** What the screen shows, such as an agent's name; the shell header already names the route. */
  readonly title: string;
  readonly lede?: ReactNode;
  /** Buttons or menus aligned with the title, at the inline end. */
  readonly actions?: ReactNode;
  readonly className?: string;
}

/** The one title block screens share inside `.ui-page`: a title, an optional lede and actions. */
export function PageHeader({ title, lede, actions, className }: PageHeaderProps) {
  invariant(title.trim().length > 0, "a page header needs a title");
  const hasActions = actions !== undefined && actions !== null && actions !== false;
  return (
    <header className={classes("ui-page-head", className)} data-actions={hasActions ? "true" : undefined}>
      <div className="ui-page-head-text">
        <h2 className="ui-page-title">{title}</h2>
        {lede === undefined ? null : <p className="ui-page-lede">{lede}</p>}
      </div>
      {hasActions ? <div className="ui-page-actions">{actions}</div> : null}
    </header>
  );
}
