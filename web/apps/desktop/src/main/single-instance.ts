export interface SingleInstanceApp {
  requestSingleInstanceLock(): boolean;
  quit(): void;
}

export function startSingleInstance(app: SingleInstanceApp, start: () => void): void {
  if (!app.requestSingleInstanceLock()) {
    app.quit();
    return;
  }
  start();
}
