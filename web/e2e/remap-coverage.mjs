// `node e2e/remap-coverage.mjs <raw-dir> <out-file>`: the browser tests' coverage
// (e2e/test.ts saves it raw, in the instrumented build's own lines and columns), merged and
// mapped back to the source files the way Vitest maps the unit tests', so `nyc report` can
// add the two up statement by statement.
//
// nyc can't be left to do this: it merges every file first and maps once after, so unit
// coverage (already in source positions) went through the build's source maps too and was
// lost. And istanbul's own mapping drops any statement whose last line has no mapping,
// which the build's JSX has plenty of; Vitest's fork of it doesn't.
import { mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { createSourceMapStore } from "@vitest/istanbul-lib-source-maps";
import libCoverage from "istanbul-lib-coverage";

const [rawDir, outFile] = process.argv.slice(2);
if (!rawDir || !outFile) {
  console.error("usage: node e2e/remap-coverage.mjs <raw-dir> <out-file>");
  process.exit(2);
}

const raw = libCoverage.createCoverageMap({});
const files = readdirSync(rawDir).filter((f) => f.endsWith(".json"));
for (const f of files) raw.merge(JSON.parse(readFileSync(join(rawDir, f), "utf8")));

const mapped = await createSourceMapStore().transformCoverage(raw);
mkdirSync(dirname(outFile), { recursive: true });
writeFileSync(outFile, JSON.stringify(mapped.toJSON()));
console.log(`${files.length} browser coverage files → ${outFile}`);
