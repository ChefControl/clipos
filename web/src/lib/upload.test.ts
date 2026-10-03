import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  BLOCK_SIZE,
  blockId,
  blockListXml,
  type UploadProgress,
  uploadToBlob,
  withParams,
} from "./upload";

describe("block upload helpers", () => {
  it("makes equal-length base64 block ids", () => {
    expect(atob(blockId(0))).toBe("block-000000");
    expect(blockId(7).length).toBe(blockId(123456).length);
  });

  it("lists blocks in order", () => {
    const xml = blockListXml(2);
    expect(xml).toContain(`<Latest>${blockId(0)}</Latest><Latest>${blockId(1)}</Latest>`);
  });

  it("adds params to a SAS URL and keeps the signature", () => {
    const sas = "https://acct.blob.core.windows.net/originals/a/b.mp4?sv=2024-11-04&sig=ab%2Bc%3D";
    const url = new URL(withParams(sas, { comp: "block", blockid: "YmxvY2s=" }));
    expect(url.searchParams.get("sig")).toBe("ab+c=");
    expect(url.searchParams.get("comp")).toBe("block");
    expect(url.searchParams.get("blockid")).toBe("YmxvY2s=");
  });
});

/** What Blob Storage does with one request: a status, a dropped connection, or nothing yet. */
type Answer = number | "network" | "hang";

/** An XMLHttpRequest the test plays Blob Storage for. */
class FakeXhr {
  static all: FakeXhr[] = [];
  static answer: (xhr: FakeXhr) => Answer = () => 201;
  method = "";
  url = "";
  headers: Record<string, string> = {};
  body: Blob | string | null = null;
  status = 0;
  aborted = false;
  upload: { onprogress: ((e: { loaded: number }) => void) | null } = { onprogress: null };
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onabort: (() => void) | null = null;

  open(method: string, url: string) {
    this.method = method;
    this.url = url;
  }
  setRequestHeader(k: string, v: string) {
    this.headers[k] = v;
  }
  send(body: Blob | string) {
    this.body = body;
    FakeXhr.all.push(this);
    const answer = FakeXhr.answer(this);
    if (answer === "hang") return;
    queueMicrotask(() => {
      if (this.aborted) return;
      if (answer === "network") return this.onerror?.();
      const size = typeof body === "string" ? body.length : body.size;
      this.upload.onprogress?.({ loaded: Math.floor(size / 2) });
      this.status = answer;
      this.onload?.();
    });
  }
  abort() {
    this.aborted = true;
    this.onabort?.();
  }

  get params() {
    return new URL(this.url).searchParams;
  }
}

const SAS = "https://acct.blob.core.windows.net/originals/a/b.mp4?sig=s";
const file = (bytes: number, type = "video/mp4") =>
  new File([new Uint8Array(bytes)], "clip.mp4", { type });
const blocks = () => FakeXhr.all.filter((x) => x.params.get("comp") === "block");
const commits = () => FakeXhr.all.filter((x) => x.params.get("comp") === "blocklist");

describe("uploading to Blob Storage", () => {
  let progress: UploadProgress[];
  const onProgress = (p: UploadProgress) => progress.push(p);

  beforeEach(() => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    FakeXhr.all = [];
    FakeXhr.answer = () => 201;
    vi.stubGlobal("XMLHttpRequest", FakeXhr);
    progress = [];
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("puts each block, then commits them in order with the file's type", async () => {
    const f = file(2 * BLOCK_SIZE + 10, "video/x-matroska");
    await uploadToBlob(f, SAS, onProgress);

    expect(blocks().map((x) => x.params.get("blockid"))).toEqual([0, 1, 2].map(blockId));
    expect(blocks().every((x) => x.method === "PUT" && x.params.get("sig") === "s")).toBe(true);
    expect(blocks().map((x) => (x.body as Blob).size)).toEqual([BLOCK_SIZE, BLOCK_SIZE, 10]);
    expect(commits()).toHaveLength(1);
    expect(commits()[0]?.body).toBe(blockListXml(3));
    expect(commits()[0]?.headers).toEqual({ "x-ms-blob-content-type": "video/x-matroska" });
    // Progress counts partial blocks as they go, and ends at the whole file.
    expect(progress.some((p) => p.loaded > 0 && p.loaded < f.size)).toBe(true);
    expect(progress.at(-1)).toEqual({ loaded: f.size, total: f.size });
  });

  it("commits a file with no type as an mp4, and an empty file as one block", async () => {
    await uploadToBlob(file(0, ""), SAS, onProgress);
    expect(blocks()).toHaveLength(1);
    expect(commits()[0]?.headers).toEqual({ "x-ms-blob-content-type": "video/mp4" });
    expect(progress.at(-1)).toEqual({ loaded: 0, total: 0 });
  });

  it("retries a failed block after 1, 2 and 4 s, and goes on when one works", async () => {
    let tries = 0;
    FakeXhr.answer = (x) => (x.params.get("comp") === "block" && ++tries < 4 ? 503 : 201);
    let done = false;
    const upload = uploadToBlob(file(100), SAS, onProgress).then(() => {
      done = true;
    });

    await vi.advanceTimersByTimeAsync(0);
    expect(blocks()).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(999);
    expect(blocks()).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(blocks()).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(2_000);
    expect(blocks()).toHaveLength(3);
    await vi.advanceTimersByTimeAsync(3_999);
    expect(blocks()).toHaveLength(3);
    await vi.advanceTimersByTimeAsync(1);
    await upload;
    expect(done).toBe(true);
    expect(blocks()).toHaveLength(4);
    expect(commits()).toHaveLength(1);
  });

  it("gives up after four tries with the last error", async () => {
    FakeXhr.answer = () => "network";
    const upload = uploadToBlob(file(100), SAS, onProgress);
    const failed = expect(upload).rejects.toThrow("network error during upload");
    await vi.advanceTimersByTimeAsync(7_000);
    await failed;
    expect(blocks()).toHaveLength(4);
    expect(commits()).toHaveLength(0);
  });

  it("retries the block list too, and says what status it got", async () => {
    FakeXhr.answer = (x) => (x.params.get("comp") === "blocklist" ? 403 : 201);
    const upload = uploadToBlob(file(100), SAS, onProgress);
    const failed = expect(upload).rejects.toThrow("upload failed (403)");
    await vi.advanceTimersByTimeAsync(7_000);
    await failed;
    expect(commits()).toHaveLength(4);
  });

  it("cancelling mid-request aborts it and doesn't retry", async () => {
    FakeXhr.answer = () => "hang";
    const controller = new AbortController();
    const upload = uploadToBlob(file(100), SAS, onProgress, controller.signal);
    const failed = expect(upload).rejects.toMatchObject({ name: "AbortError" });
    await vi.advanceTimersByTimeAsync(0);
    controller.abort();
    await failed;
    expect(blocks()[0]?.aborted).toBe(true);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(FakeXhr.all).toHaveLength(1);
  });

  it("cancelling while it waits to retry stops at once", async () => {
    FakeXhr.answer = () => 500;
    const controller = new AbortController();
    const upload = uploadToBlob(file(100), SAS, onProgress, controller.signal);
    const failed = expect(upload).rejects.toBe("stop");
    await vi.advanceTimersByTimeAsync(500);
    controller.abort("stop");
    await failed;
    await vi.advanceTimersByTimeAsync(60_000);
    expect(FakeXhr.all).toHaveLength(1);
  });

  it("an upload cancelled before it starts sends nothing", async () => {
    const controller = new AbortController();
    controller.abort();
    await expect(uploadToBlob(file(100), SAS, onProgress, controller.signal)).rejects.toMatchObject(
      {
        name: "AbortError",
      },
    );
    expect(FakeXhr.all).toHaveLength(0);
  });
});
