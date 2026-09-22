#!/usr/bin/env node
// diffcase.mjs <binary> <case-name>... — show byte diff for specific golden cases.
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const goldenDir = path.join(path.dirname(fileURLToPath(import.meta.url)), "golden-args");
const [bin, ...cases] = process.argv.slice(2);
const env = { ...process.env, NO_COLOR: "1", FORCE_COLOR: "0", CI: "1", TERM: "dumb", LC_ALL: "C", LANG: "C", TZ: "UTC" };
for (const name of cases) {
  const { argv } = JSON.parse(
    readFileSync(path.join(goldenDir, "manifest.jsonl"), "utf8")
      .split("\n")
      .find((l) => l.includes(`"name":"${name}"`)),
  );
  const r = spawnSync(bin, argv, { env, encoding: "buffer" });
  const expOut = readFileSync(path.join(goldenDir, `${name}.out`));
  const expErr = readFileSync(path.join(goldenDir, `${name}.err`));
  const expCode = Number(readFileSync(path.join(goldenDir, `${name}.exit`), "utf8").trim());
  console.log(`== ${name} argv=${JSON.stringify(argv)} rustExit=${r.status} goldExit=${expCode}`);
  if (!r.stdout.equals(expOut)) {
    console.log("stdout rust:", JSON.stringify(r.stdout.toString("utf8").slice(0, 300)));
    console.log("stdout gold:", JSON.stringify(expOut.toString("utf8").slice(0, 300)));
  }
  if (!r.stderr.equals(expErr)) {
    const rs = r.stderr.toString("utf8");
    const gs = expErr.toString("utf8");
    for (let i = 0; i < Math.max(rs.length, gs.length); i++) {
      if (rs[i] !== gs[i]) {
        console.log(`stderr first diff @${i}:`);
        console.log("  rust:", JSON.stringify(rs.slice(Math.max(0, i - 40), i + 60)));
        console.log("  gold:", JSON.stringify(gs.slice(Math.max(0, i - 40), i + 60)));
        break;
      }
    }
  }
}
