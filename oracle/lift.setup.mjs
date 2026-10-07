// Appends one JSON line per direct test call to a row-level CsvValidator method: the test
// name, the argument, and the validator state the method reads. Calls made by validate()
// itself come from fixtures, which the oracle already covers, so they are skipped.
import fs from "node:fs";
import path from "node:path";

// Some tests shuffle columns with lodash, which uses Math.random. A seeded generator
// (mulberry32), reset for every test file, makes the lifted files reproducible.
globalThis.liftSeed = 1;
if (!Math.random.seeded) {
  Math.random = () => {
    let t = (globalThis.liftSeed = (globalThis.liftSeed + 0x6d2b79f5) | 0);
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  Math.random.seeded = true;
}

const { CsvValidator } = await import(path.join(process.cwd(), "src/validators/CsvValidator.ts"));
const METHODS = ["validateHeaderColumns", "validateHeaderRow", "alertHeaderRow", "validateColumns", "alertColumns",
  "validateDataRow", "alertDataRow"];

let depth = 0;
const validate = CsvValidator.prototype.validate;
CsvValidator.prototype.validate = function (...args) {
  this.liftSkip = true;
  return validate.apply(this, args);
};
for (const method of METHODS) {
  const original = CsvValidator.prototype[method];
  CsvValidator.prototype[method] = function (arg) {
    if (depth === 0 && !this.liftSkip) {
      fs.appendFileSync(process.env.LIFT_OUT, JSON.stringify({
        test: expect.getState().currentTestName, method, arg, version: this.version,
        headerColumns: this.headerColumns, dataColumns: this.dataColumns,
        normalizedColumns: this.normalizedColumns,
      }) + "\n");
    }
    depth++;
    try {
      return original.call(this, arg);
    } finally {
      depth--;
    }
  };
}
