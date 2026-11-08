# Desktop bridge contract amendment

## §2.4: unexpected local connection closure

`DesktopBridge.onClose(conn: string, cb: () => void): () => void` registers a
callback for an unexpected closure of the local connection owned by this window.
The return value removes the callback. An explicit `close` does not notify it.
A closure before registration may instead be detected by the next bridge request,
which fails for a closed connection. Main sends the event only to the owner.

The complete TypeScript declarations are in `src/app.ts` and the contract fixture
in `test/fixtures/contracts-app-types.ts.txt`.
