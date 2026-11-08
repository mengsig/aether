interface OwnerContents {
  isDestroyed(): boolean;
  send(channel: string, ...args: unknown[]): void;
}

/** Delivers only to the recorded owner and skips a window that has already gone away. */
export function sendToOwner(find: (id: number) => OwnerContents | undefined, owner: number, channel: string, ...args: unknown[]): boolean {
  const contents = find(owner);
  if (contents === undefined || contents.isDestroyed()) return false;
  contents.send(channel, ...args);
  return true;
}
