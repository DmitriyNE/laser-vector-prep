import { describe, expect, it } from "vitest";
import { transferables, type Payload, type Request } from "./protocol.js";

describe("worker protocol", () => {
  it("keeps the request id apart from payload ids", () => {
    const payload: Payload = { kind: "previewLayer", id: "wl-copper", toleranceMm: 0.01 };
    const req: Request = { rid: 42, payload };
    expect(req.payload.kind === "previewLayer" && req.payload.id).toBe("wl-copper");
    expect(req.rid).toBe(42);
  });

  it("transfers file bytes and nothing else", () => {
    const bytes = new Uint8Array([1, 2, 3]);
    expect(transferables({ kind: "addSource", name: "a.dxf", bytes })).toEqual([bytes.buffer]);
    expect(transferables({ kind: "evaluate" })).toEqual([]);
    expect(transferables({ kind: "previewPass", id: "p", toleranceMm: 0.01 })).toEqual([]);
  });
});
