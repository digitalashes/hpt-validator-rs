// Writes upstream's result for every file in tests/cases to tests/golden/<same path>.json.
// Input goes through fs.createReadStream(path, "utf-8"), the CLI's path: 64 KiB chunks.
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const upstream = path.resolve(process.argv[2]);
const { validateCsv, validateJson } = await import(pathToFileURL(path.join(upstream, "lib/index.js")));
const VERSIONS = ["v2.0.0", "v2.1.0", "v2.2.0", "v3.0.0"];
const LIMITS = [0, 2];
// Cases under versions/ also run with these, to pin coercion and unknown versions (F-22).
const ODD_VERSIONS = ["x.y.z", "3", "2.2.1", "03.0.0"];

let count = 0;
for (const file of fs.readdirSync("tests/cases", { recursive: true }).sort()) {
  const ext = path.extname(file);
  if (ext !== ".csv" && ext !== ".json") continue;
  const validate = ext === ".csv" ? validateCsv : validateJson;
  const golden = {};
  const unit = file.match(/^unit\/(v[\d.]+)\//); // lifted unit tests: their own version only
  for (const version of unit ? [unit[1]] : file.startsWith("versions") ? [...VERSIONS, ...ODD_VERSIONS] : VERSIONS) {
    for (const maxErrors of LIMITS) {
      const input = fs.createReadStream(path.join("tests/cases", file), "utf-8");
      golden[`${version} maxErrors=${maxErrors}`] = await validate(input, version, { maxErrors })
        .catch((e) => ({ throws: String(e) }));
    }
  }
  const out = path.join("tests/golden", file + ".json");
  fs.mkdirSync(path.dirname(out), { recursive: true });
  fs.writeFileSync(out, JSON.stringify(golden, null, 2) + "\n");
  count++;
}
console.log(`${count} cases`);
