/**
 * Uploads a file straight to Azure Blob Storage with a write SAS, as a block blob:
 * 8 MiB blocks, 4 in flight, each retried, then a block list commits them. Progress
 * comes from XHR upload events (fetch has none).
 */

export const BLOCK_SIZE = 8 * 1024 * 1024;
const CONCURRENCY = 4;
const ATTEMPTS = 4;

export interface UploadProgress {
  loaded: number;
  total: number;
}

/** Block ids must be base64 and all the same length within a blob. */
export function blockId(index: number): string {
  return btoa(`block-${index.toString().padStart(6, "0")}`);
}

export function blockListXml(count: number): string {
  const ids = Array.from({ length: count }, (_, i) => `<Latest>${blockId(i)}</Latest>`);
  return `<?xml version="1.0" encoding="utf-8"?><BlockList>${ids.join("")}</BlockList>`;
}

/** Adds query parameters to a SAS URL without disturbing its signature. */
export function withParams(sasUrl: string, params: Record<string, string>): string {
  const url = new URL(sasUrl);
  for (const [k, v] of Object.entries(params)) {
    url.searchParams.set(k, v);
  }
  return url.toString();
}

export async function uploadToBlob(
  file: File,
  sasUrl: string,
  onProgress: (p: UploadProgress) => void,
  signal?: AbortSignal,
): Promise<void> {
  const count = Math.max(1, Math.ceil(file.size / BLOCK_SIZE));
  const loadedPerBlock = new Array<number>(count).fill(0);
  const report = () =>
    onProgress({ loaded: loadedPerBlock.reduce((a, b) => a + b, 0), total: file.size });

  let next = 0;
  const worker = async () => {
    while (next < count) {
      const index = next++;
      const start = index * BLOCK_SIZE;
      const chunk = file.slice(start, Math.min(start + BLOCK_SIZE, file.size));
      const url = withParams(sasUrl, { comp: "block", blockid: blockId(index) });
      await withRetries(
        () =>
          put(url, chunk, {}, signal, (loaded) => {
            loadedPerBlock[index] = loaded;
            report();
          }),
        signal,
      );
      loadedPerBlock[index] = chunk.size;
      report();
    }
  };
  await Promise.all(Array.from({ length: Math.min(CONCURRENCY, count) }, worker));

  await withRetries(
    () =>
      put(
        withParams(sasUrl, { comp: "blocklist" }),
        blockListXml(count),
        { "x-ms-blob-content-type": file.type || "video/mp4" },
        signal,
      ),
    signal,
  );
}

/** Cancelling stops it between tries too, not only mid-request. */
async function withRetries(attempt: () => Promise<void>, signal?: AbortSignal): Promise<void> {
  for (let i = 1; ; i++) {
    signal?.throwIfAborted();
    try {
      return await attempt();
    } catch (e) {
      if (
        i >= ATTEMPTS ||
        signal?.aborted ||
        (e instanceof DOMException && e.name === "AbortError")
      ) {
        throw e;
      }
      await sleep(1000 * 2 ** (i - 1), signal);
    }
  }
}

/** Waits `ms`, or rejects as soon as `signal` aborts. */
function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    const stop = () => {
      clearTimeout(timer);
      reject(signal?.reason);
    };
    const timer = setTimeout(() => {
      signal?.removeEventListener("abort", stop);
      resolve();
    }, ms);
    signal?.addEventListener("abort", stop, { once: true });
  });
}

function put(
  url: string,
  body: Blob | string,
  headers: Record<string, string>,
  signal?: AbortSignal,
  onProgress?: (loaded: number) => void,
): Promise<void> {
  // A request that hasn't started yet doesn't start after a cancel.
  signal?.throwIfAborted();
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open("PUT", url);
    for (const [k, v] of Object.entries(headers)) {
      xhr.setRequestHeader(k, v);
    }
    xhr.upload.onprogress = (e) => onProgress?.(e.loaded);
    xhr.onload = () =>
      xhr.status >= 200 && xhr.status < 300
        ? resolve()
        : reject(new Error(`upload failed (${xhr.status})`));
    xhr.onerror = () => reject(new Error("network error during upload"));
    xhr.onabort = () => reject(new DOMException("upload cancelled", "AbortError"));
    signal?.addEventListener("abort", () => xhr.abort(), { once: true });
    xhr.send(body);
  });
}
