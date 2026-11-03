// The error a daemon answered with, keeping its code and cursor details.

import { object, string, type Json } from "./guards.ts";

export class DaemonError extends Error {
  readonly code: string;
  readonly details: Json;

  constructor(code: string, message: string, details: Json = {}, options?: ErrorOptions) {
    super(message, options);
    this.name = "DaemonError";
    this.code = code;
    this.details = details;
  }

  static fromReply(value: unknown): DaemonError {
    const body = object(value, "error reply");
    return new DaemonError(string(body.code, "error.code"), String(body.message ?? body.code), body);
  }
}

/** A local refusal before anything is sent: the role or host does not allow it. */
export class RefusedError extends Error {
  readonly code = "forbidden";
  constructor(message: string) {
    super(message);
    this.name = "RefusedError";
  }
}

export const codeOf = (error: unknown): string | undefined =>
  error instanceof DaemonError || error instanceof RefusedError ? error.code : undefined;
