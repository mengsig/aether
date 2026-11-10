import { navigate, useCapabilities, useRoutes, useSelector } from "@aether-ui/core";
import { CommandPalette, type CommandPaletteProps } from "@aether-ui/ui";
import { useMemo } from "react";
import { entityCommands, routeOffered, selectPaletteSources, sameSources } from "./palette-entities.ts";

/** Builds entity destinations only while the palette is open. */
export function EntityPalette(props: CommandPaletteProps) {
  const routes = useRoutes();
  const capabilities = useCapabilities();
  const sources = useSelector(selectPaletteSources, sameSources);
  const entities = useMemo(
    () => entityCommands(sources, { navigate, offered: (pattern) => routeOffered(routes, capabilities, pattern) }),
    [sources, routes, capabilities],
  );
  const commands = useMemo(() => [...props.commands, ...entities], [props.commands, entities]);
  return <CommandPalette {...props} commands={commands} />;
}
