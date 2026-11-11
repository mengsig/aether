# Aether remote web

Static, dependency-free ES modules. Runtime files are served without a build step.

## Design

An unhurried control room: warm white / ink surfaces, restrained elevation, emerald for live health, amber only for attention. The delegation graph is the fleet's primary surface; details open in a sheet. Conversations use the full viewport with a keyboard-aware composer. Native system type; tabular numbers; 44px minimum touch targets. Large titles collapse on scroll; the mobile content viewport reserves measured space for the translucent navigation. Body is 16px, metadata 13px. Controls use 220ms transitions and page entry uses 350ms; all motion respects reduced motion. CSS variables are the only palette source.

## Architecture and feature boundaries

- `core/`: protocol boundary, device identity, socket, normalized store and persistence.
- `features/fleet/`: agent tree/list, activity and attention inbox.
- `features/conversation/`: transcript, safe markdown, virtualisation, composer.
- `features/controls/`: agent operations, memories/artifacts/schedules/reports and settings.
- `features/start/`: pairing, installation and starting work.
- `ui/`: safe DOM primitives and icons. `app.js`: shell, route lifetime and integration.
- `dev/`: mock HTTP/WebSocket daemon. `tests/`: Node unit and browser tests.

Every screen exports `mount(container, context)` returning a cleanup function. Context is defined in `types.js`; screens subscribe to the store and own updates without replacing the entire app. Routes: `fleet`, `attention`, `activity`, `agent/:id`, `controls/:id`, `settings`, `new`, `pair`.

Wire commands retain protocol names and fields. `context.command(command)` provides a mutation idempotency key, role enforcement, error reporting and refresh. `context.transport.request(command)` is for reads. Replies are `{reply,data?}` after unwrapping Rust Result. `context.store.get()` returns normalized state; `subscribe(listener)` returns unsubscribe. Rendering uses DOM APIs, never innerHTML. Unknown costs remain unknown. Persisted sequence advances only after application; deltas are transient and cleared/reconciled on reconnect.

No cargo, bundler or global tooling caches. Keep node_modules, npm cache, browsers and test output inside this directory. Commit only owned files as the configured git user, with no attribution trailers. Functions under 60 lines, useful invariant checks at boundaries, no swallowed errors. All contributors own disjoint paths.
