/**
 * A file's fingerprint, to ask whether it's here already before uploading it (decision 55;
 * the server's side is crates/core/src/dedup.rs, which computes exactly the same hashes):
 *
 * - the sample hash: SHA-256 of three 1 MiB samples (the start, the middle and the end), or
 *   of the whole file when it's no bigger than that. Milliseconds, whatever the file's size.
 * - the content hash: SHA-256 of the SHA-256 of each 8 MiB block, in order. It reads all
 *   of the file, so it's only computed when the samples match something. Web Crypto hashes
 *   natively, a buffer at a time, so the blocks are hashed four at once.
 */
import type { components } from "../api/schema";

export const SAMPLE_SIZE = 1024 * 1024;
export const HASH_BLOCK = 8 * 1024 * 1024;
const CONCURRENCY = 4;

export type CheckUpload = components["schemas"]["CheckUpload"];
export type UploadCheck = components["schemas"]["UploadCheck"];

async function sha256(data: BufferSource): Promise<Uint8Array> {
  return new Uint8Array(await crypto.subtle.digest("SHA-256", data));
}

export function hex(bytes: Uint8Array): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

/** The [start, end) byte ranges the sample hash reads, in order. */
export function sampleRanges(size: number): [number, number][] {
  if (size <= 3 * SAMPLE_SIZE) return [[0, size]];
  const middle = Math.floor((size - SAMPLE_SIZE) / 2);
  return [
    [0, SAMPLE_SIZE],
    [middle, middle + SAMPLE_SIZE],
    [size - SAMPLE_SIZE, size],
  ];
}

export async function sampleHash(file: Blob): Promise<string> {
  const parts = sampleRanges(file.size).map(([start, end]) => file.slice(start, end));
  return hex(await sha256(await new Blob(parts).arrayBuffer()));
}

/** `onProgress` gets the bytes hashed so far. Cancelling stops it between blocks. */
export async function contentHash(
  file: Blob,
  onProgress?: (hashed: number) => void,
  signal?: AbortSignal,
): Promise<string> {
  const count = Math.ceil(file.size / HASH_BLOCK);
  const digests = new Uint8Array(count * 32);
  let next = 0;
  let hashed = 0;
  const worker = async () => {
    while (next < count) {
      signal?.throwIfAborted();
      const index = next++;
      const block = file.slice(index * HASH_BLOCK, (index + 1) * HASH_BLOCK);
      digests.set(await sha256(await block.arrayBuffer()), index * 32);
      hashed += block.size;
      onProgress?.(hashed);
    }
  };
  await Promise.all(Array.from({ length: Math.min(CONCURRENCY, count) }, worker));
  return hex(await sha256(digests));
}

/**
 * Whether `file` is here already, asked with `ask` (`POST /api/clips/check`): by its size
 * and samples, and only when another file shares those, by all of it too. `onHashing`
 * hears how much of it that's hashed (0 to 1).
 */
export async function findCopy(
  file: Blob,
  ask: (body: CheckUpload) => Promise<UploadCheck>,
  onHashing?: (fraction: number) => void,
  signal?: AbortSignal,
): Promise<UploadCheck> {
  const body = { bytes: file.size, sampleHash: await sampleHash(file) };
  signal?.throwIfAborted();
  const first = await ask(body);
  if (first.result !== "verify") return first;
  onHashing?.(0);
  const content = await contentHash(file, (hashed) => onHashing?.(hashed / file.size), signal);
  return ask({ ...body, contentHash: content });
}
