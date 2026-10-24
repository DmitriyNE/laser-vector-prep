// Message types between the main thread and the geometry worker. Small structured data
// travels as parsed JSON objects; geometry and files as transferable typed arrays.

import type { EvaluationSummary, GroupPdf, Preview, Profile, SourceSummary, Diagnostic } from "../core/types.js";

export type Payload =
  | { kind: "version" }
  | { kind: "threads" }
  | { kind: "presets" }
  | { kind: "addSource"; name: string; bytes: Uint8Array }
  | { kind: "replaceSource"; index: number; bytes: Uint8Array }
  | { kind: "acceptedExtensions" }
  | { kind: "removeSource"; index: number }
  | { kind: "sources" }
  | { kind: "setProfile"; profile: Profile }
  | { kind: "getProfile" }
  | { kind: "validateProfile"; profile: Profile }
  | { kind: "evaluate" }
  | { kind: "previewPass"; id: string; toleranceMm: number }
  | { kind: "previewLayer"; id: string; toleranceMm: number }
  | { kind: "previewSource"; file: number; layer: string; toleranceMm: number }
  | { kind: "previewJumps"; id: string }
  | { kind: "exportPdf" }
  | { kind: "exportPdfPerGroup" };

export type Kind = Payload["kind"];

export type ResultOf<K extends Kind> = K extends "version"
  ? string
  : K extends "threads"
    ? number
    : K extends "presets"
      ? Profile[]
      : K extends "acceptedExtensions"
        ? string[]
      : K extends "addSource" | "replaceSource"
        ? SourceSummary
        : K extends "removeSource"
          ? void
          : K extends "sources"
            ? SourceSummary[]
            : K extends "setProfile"
              ? void
              : K extends "getProfile"
                ? Profile
                : K extends "validateProfile"
                  ? Diagnostic[]
                  : K extends "evaluate"
                    ? EvaluationSummary
                    : K extends "previewPass" | "previewLayer" | "previewSource" | "previewJumps"
                      ? Preview | null
                      : K extends "exportPdf"
                        ? Uint8Array
                        : K extends "exportPdfPerGroup"
                          ? GroupPdf[]
                          : never;

/**
 * What actually travels to the worker. The request id lives in its own field: several
 * payloads carry an `id` of their own (layer, pass), so the envelope never spreads them.
 */
export interface Request {
  rid: number;
  payload: Payload;
}

export type Response =
  | { rid: number; ok: true; result: unknown }
  /** `fatal` means the wasm instance trapped and must be replaced. */
  | { rid: number; ok: false; error: string; fatal: boolean };

/** Buffers in a payload that should be transferred rather than copied. */
export function transferables(p: Payload): Transferable[] {
  return "bytes" in p ? [p.bytes.buffer] : [];
}
