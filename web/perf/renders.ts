// Render counters the scenarios' module mocks bump: each wraps one component a row draws once,
// so a count is how many rows React rendered, whatever the timing.

export const renders = { pill: 0, message: 0 };

export function resetRenders(): void {
  renders.pill = 0;
  renders.message = 0;
}
