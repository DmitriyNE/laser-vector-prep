/** Trigger a browser download of in-memory data. */
export function download(name: string, data: Uint8Array | string, type: string) {
  // Bytes from the worker may live on a SharedArrayBuffer; Blob needs a plain copy.
  const part: BlobPart = typeof data === "string" ? data : new Uint8Array(data);
  const url = URL.createObjectURL(new Blob([part], { type }));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

export function shortId(prefix: string): string {
  return `${prefix}-${crypto.randomUUID().slice(0, 8)}`;
}
