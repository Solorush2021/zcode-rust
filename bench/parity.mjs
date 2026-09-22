#!/usr/bin/env node
// parity.mjs — G2 gate: replay golden cases against a binary, byte-compare
// stdout/stderr/exit. Manifest: manifest.jsonl {name, argv, mode}.
// mode: check = strict byte parity; mask = parity except runtime-dependent
// lines (node:/execPath:/cwd: prefixes and their JSON keys); skip = excluded.
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const [binArg, goldenDirArg] = process.argv.slice(2);
const bin = path.resolve(binArg);
const goldenDir = goldenDirArg || path.join(path.dirname(fileURLToPath(import.meta.url)), "golden-args");
const manifest = readFileSync(path.join(goldenDir, "manifest.jsonl"), "utf8")
  .split("\n")
  .filter((l) => l.trim());

const env = {
  ...process.env,
  NO_COLOR: "1",
  FORCE_COLOR: "0",
  CI: "1",
  TERM: "dumb",
  LC_ALL: "C",
  LANG: "C",
  TZ: "UTC",
};
// Vendored-tool overrides (ZCODE_UGREP_BINARY/ZCODE_BFS_BINARY/ZCODE_RG_BINARY)
// change which binary __internal-search spawns (mirrors the TS backend selector),
// so goldens are only reproducible in sessions without them. Strip from the base
// env; manifest rows re-add them explicitly via their own `env` object.
for (const k of ["ZCODE_UGREP_BINARY", "ZCODE_BFS_BINARY", "ZCODE_RG_BINARY"]) {
  delete env[k];
}

const maskLine = (l) =>
  /^(node|execPath|cwd|default artifact):/.test(l) ||
  /^\s*"(execPath|cwd|node|default)"\s*:/.test(l);

let pass = 0;
let fail = 0;
const failures = [];
for (const line of manifest) {
  const { name, argv, mode, cwd, env: caseEnv } = JSON.parse(line);
  if (mode === "skip") continue;
  // Optional per-case env override: merged over the base env (backward-compatible —
  // rows without `env` behave exactly as before).
  const runEnv = caseEnv ? { ...env, ...caseEnv } : env;
  const r = spawnSync(bin, argv, { env: runEnv, encoding: "buffer", timeout: 30000, cwd });
  const exp = {
    out: readFileSync(path.join(goldenDir, `${name}.out`)),
    err: readFileSync(path.join(goldenDir, `${name}.err`)),
    code: Number(readFileSync(path.join(goldenDir, `${name}.exit`), "utf8").trim()),
  };
  let ok = r.status === exp.code;
  if (mode === "mask") {
    const mask = (b) => b.toString("utf8").split("\n").filter((l) => !maskLine(l)).join("\n");
    ok = ok && mask(r.stdout) === mask(exp.out) && mask(r.stderr) === mask(exp.err);
  } else {
    ok = ok && r.stdout.equals(exp.out) && r.stderr.equals(exp.err);
  }
  if (ok) pass += 1;
  else {
    fail += 1;
    failures.push(name);
  }
}
console.log(`parity: ${pass} pass, ${fail} fail`);
if (failures.length) console.log("failed:", failures.join(" "));
process.exit(fail === 0 ? 0 : 1);
