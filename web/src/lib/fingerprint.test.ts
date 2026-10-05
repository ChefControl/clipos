import { describe, expect, it, vi } from "vitest";
import {
  type CheckUpload,
  contentHash,
  findCopy,
  HASH_BLOCK,
  hex,
  SAMPLE_SIZE,
  sampleHash,
  sampleRanges,
  type UploadCheck,
} from "./fingerprint";

const MiB = 1024 * 1024;

/** The test data crates/core/src/dedup.rs hashes too. */
function data(length: number): Blob {
  const bytes = new Uint8Array(length);
  for (let i = 0; i < length; i++) bytes[i] = (i * 31 + 7) % 251;
  return new Blob([bytes]);
}

// The same files' fingerprints in crates/core/src/dedup.rs, so the browser and the worker
// agree on every one.
const VECTORS: [number, string, string][] = [
  [
    1000,
    "008549d94fa71e7a0a483d84380d05a923a4b18e79ba1f8a8ddac923956d32ef",
    "a5f998fc390196f6d0c72a3494f38548e21d312ec821426b73b31f5e192ba8eb",
  ],
  [
    3 * MiB,
    "9c858aecb2673a768ee641b231391f0be6189325d64cb0a68f0e4e1be9bfa68b",
    "bea0986c2a69fa1f209ecc540a4d716cd1dfcf43cda3302c9c6ba29f23cb7635",
  ],
  [
    3 * MiB + 1,
    "009a6c4a78500ab777302ec92c4c0a11acc574380e8cb9d30be2213169d1b214",
    "566ab5cdde919eb3f3969c00d940c6363a53b348bb338fa48272051890ebf1d8",
  ],
  [
    20 * MiB + 123,
    "13e52089fb34511154b7861b29916b17349c828d59fcd6aa0b4ba2e3213bbee9",
    "225cf47c7227ab4ac8eefbfdf00550125ff03073c4aaa797e25f4dd875cf5a48",
  ],
];

describe("fingerprints", () => {
  it.each(VECTORS)("of %i bytes match the worker's", async (length, sample, content) => {
    const file = data(length);
    expect(await sampleHash(file)).toBe(sample);
    expect(await contentHash(file)).toBe(content);
  });

  it("samples the start, the middle and the end of a file over 3 MiB", () => {
    expect(SAMPLE_SIZE).toBe(MiB);
    expect(sampleRanges(1000)).toEqual([[0, 1000]]);
    expect(sampleRanges(3 * MiB)).toEqual([[0, 3 * MiB]]);
    expect(sampleRanges(3 * MiB + 1)).toEqual([
      [0, MiB],
      [MiB, 2 * MiB],
      [2 * MiB + 1, 3 * MiB + 1],
    ]);
  });

  it("reports what it has hashed, block by block", async () => {
    const hashed: number[] = [];
    await contentHash(data(2 * HASH_BLOCK + 5), (n) => hashed.push(n));
    expect(hashed.sort((a, b) => a - b)).toEqual([
      expect.any(Number),
      expect.any(Number),
      2 * HASH_BLOCK + 5,
    ]);
  });

  it("stops when cancelled", async () => {
    const controller = new AbortController();
    controller.abort(new DOMException("cancelled", "AbortError"));
    await expect(contentHash(data(1000), undefined, controller.signal)).rejects.toThrow(
      "cancelled",
    );
  });

  it("writes bytes as lower-case hex", () => {
    expect(hex(new Uint8Array([0, 15, 16, 255]))).toBe("000f10ff");
  });
});

describe("findCopy", () => {
  const file = data(1000);
  const [, sample, content] = VECTORS[0] as [number, string, string];

  it("asks with the samples, and that's it unless they match something", async () => {
    const ask = vi.fn(async (): Promise<UploadCheck> => ({ result: "new", clip: null }));
    const hashing = vi.fn();
    expect(await findCopy(file, ask, hashing)).toEqual({ result: "new", clip: null });
    expect(ask.mock.calls).toEqual([[{ bytes: 1000, sampleHash: sample }]]);
    expect(hashing).not.toHaveBeenCalled();
  });

  it("asks again with all of it when they do", async () => {
    const answers: UploadCheck[] = [
      { result: "verify", clip: null },
      { result: "duplicate", clip: null },
    ];
    const asked: CheckUpload[] = [];
    const ask = async (body: CheckUpload) => {
      asked.push(body);
      return answers.shift() as UploadCheck;
    };
    const hashing: number[] = [];
    const found = await findCopy(file, ask, (f) => hashing.push(f));
    expect(found.result).toBe("duplicate");
    expect(asked).toEqual([
      { bytes: 1000, sampleHash: sample },
      { bytes: 1000, sampleHash: sample, contentHash: content },
    ]);
    expect(hashing).toEqual([0, 1]);
    // Without anyone listening, too.
    answers.push({ result: "verify", clip: null }, { result: "new", clip: null });
    expect((await findCopy(file, ask)).result).toBe("new");
  });

  it("asks nothing once cancelled", async () => {
    const controller = new AbortController();
    controller.abort();
    const ask = vi.fn();
    await expect(findCopy(file, ask, undefined, controller.signal)).rejects.toBeDefined();
    expect(ask).not.toHaveBeenCalled();
  });
});
