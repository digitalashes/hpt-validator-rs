// Writes tests/cases/findings: one or more input files per visible upstream finding
// (docs: upstream-findings F-n), tests/cases/rules for data-row rule edges, and
// tests/cases/versions for version-string handling.
// The oracle then records upstream's result for each, like any other case.
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const upstream = path.resolve(process.argv[2]);
const { AFFIRMATION, ATTESTATION } = await import(pathToFileURL(path.join(upstream, "lib/validators/CsvHelpers.js")));
const fixture = (name) => JSON.parse(fs.readFileSync(path.join(upstream, "test/fixtures", name), "utf-8"));

const write = (file, content) => {
  const out = path.join("tests/cases", file);
  fs.mkdirSync(path.dirname(out), { recursive: true });
  fs.writeFileSync(out, content);
};
const field = (v) => (/[",\n\r]/.test(v) ? `"${v.replaceAll('"', '""')}"` : v);
const csv = (rows) => rows.map((r) => r.map(field).join(",")).join("\n") + "\n";
const json = (doc) => JSON.stringify(doc, null, 2) + "\n";

// --- CSV templates ------------------------------------------------------------------

const v3 = (version) => version.startsWith("3");

// Rows 1-2. `values` overrides a value by column name, `rename` renames a column.
function header(version, values = {}, rename = {}) {
  const cols = v3(version)
    ? ["hospital_name", "last_updated_on", "version", "location_name", "hospital_address", "license_number | CA",
      "type_2_npi", ATTESTATION, "attester_name"]
    : ["hospital_name", "last_updated_on", "version", "hospital_location", "hospital_address", "license_number | CA",
      AFFIRMATION];
  const all = {
    hospital_name: "West Mercy Hospital", last_updated_on: "2024-07-01", version, location_name: "West Mercy Hospital",
    hospital_location: "West Mercy Hospital", hospital_address: "12 Main Street", "license_number | CA": "50056",
    type_2_npi: "1122334455", [ATTESTATION]: "true", attester_name: "Jane Doe", [AFFIRMATION]: "true", ...values,
  };
  return [cols.map((c) => rename[c] ?? c), cols.map((c) => all[c] ?? "")];
}

const BASE = ["description", "code | 1", "code | 1 | type", "setting"];
const AMOUNTS = ["standard_charge | gross", "standard_charge | discounted_cash"];
const TAIL = ["standard_charge | min", "standard_charge | max", "additional_generic_notes"];
const drug = (version) => (version >= "2.2" ? ["drug_unit_of_measurement", "drug_type_of_measurement", "modifiers"] : []);
const allowed = (s) => ["median_amount" + s, "10th_percentile" + s, "90th_percentile" + s, "count" + s];
const extra = (version, s = "") =>
  version.startsWith("2.2") ? ["estimated_amount" + s] : v3(version) ? allowed(s) : [];

function tallColumns(version) {
  return [...BASE, ...drug(version), ...AMOUNTS, "payer_name", "plan_name", "standard_charge | negotiated_dollar",
    "standard_charge | negotiated_percentage", "standard_charge | negotiated_algorithm", "standard_charge | methodology",
    ...extra(version), ...TAIL];
}

function wideColumns(version, payers) {
  return [...BASE, ...drug(version), ...AMOUNTS, ...payers.flatMap((p) => [
    `standard_charge | ${p} | negotiated_dollar`, `standard_charge | ${p} | negotiated_percentage`,
    `standard_charge | ${p} | negotiated_algorithm`, `standard_charge | ${p} | methodology`,
    `additional_payer_notes | ${p}`, ...extra(version, ` | ${p}`)]), ...TAIL];
}

// A charge row that upstream accepts; `values` overrides by column name.
function item(cols, values = {}) {
  const all = {
    description: "Hip replacement", "code | 1": "470", "code | 1 | type": "MS-DRG", setting: "inpatient",
    "standard_charge | gross": "25000", "standard_charge | discounted_cash": "20000", payer_name: "Payer A",
    plan_name: "PPO", "standard_charge | min": "18000", "standard_charge | max": "18000", ...values,
  };
  return cols.map((c) => {
    if (c in all) return all[c];
    const charge = c.match(/^(standard_charge \| (?:.+ \| )?)(negotiated_dollar|methodology)$/);
    if (charge) return values[c] ?? (charge[2] === "methodology" ? "case rate" : "18000");
    return "";
  });
}

const tall = (version, rows = [{}], h = header(version)) => {
  const cols = tallColumns(version);
  return [...h, cols, ...rows.map((r) => item(cols, r))];
};
const wide = (version, payers, rows = [{}], h = header(version)) => {
  const cols = wideColumns(version, payers);
  return [...h, cols, ...rows.map((r) => item(cols, r))];
};

// Templates first: every finding case below is one of these with a single defect.
for (const version of ["2.0.0", "2.2.0", "3.0.0"]) {
  write(`findings/template-tall-${version}.csv`, csv(tall(version)));
  write(`findings/template-wide-${version}.csv`, csv(wide(version, ["Payer A | PPO"])));
}

// F-1, F-11: a blank row 1 (no BOM) fails; the same file with a BOM is valid, one row lower.
write("findings/f01-blank-row1.csv", "\n" + csv(tall("2.0.0")));
write("findings/f11-bom-blank-row1.csv", "﻿\n" + csv(tall("2.0.0")));
// F-1, F-15: a blank row 3 returns only HeaderBlankError, dropping the row-2 date error.
{
  const [h, v, ...rest] = tall("2.0.0", [{}], header("2.0.0", { last_updated_on: "July 1 2024" }));
  write("findings/f15-blank-row3.csv", csv([h, v]) + "\n" + csv(rest.slice(1)));
}
// F-2, F-23: a CSV declaring 2.2.0 gets VersionMismatchAlert anchored at row 3; JSON does not.
write("findings/f02-version-2.2.0.csv", csv(tall("2.2.0", [{}], header("2.2.0"))));
write("findings/f02-version-2.2.0.json", json({ ...fixture("sample-valid.json"), version: "2.2.0" }));
// F-6: years below 100.
write("findings/f06-slash-year-50.csv", csv(tall("2.0.0", [{}], header("2.0.0", { last_updated_on: "1/1/0050" }))));
write("findings/f06-iso-year-50.csv", csv(tall("2.0.0", [{}], header("2.0.0", { last_updated_on: "0050-01-01" }))));
// F-13: a tall modifier row without information names additional_payer_notes.
write("findings/f13-modifier-tall.csv", csv(tall("3.0.0", [{}, {
  "code | 1": "", "code | 1 | type": "", modifiers: "50", payer_name: "", plan_name: "",
  "standard_charge | negotiated_dollar": "", "standard_charge | methodology": "", "standard_charge | min": "",
  "standard_charge | max": "", "standard_charge | gross": "", "standard_charge | discounted_cash": "",
}])));
// F-14: more missing columns than the limit, then ProblemsInHeaderError on top.
{
  const [h, v] = header("2.0.0");
  write("findings/f14-many-missing-columns.csv", csv([h, v, ["description", "payer_name", "plan_name"], ["x", "a", "b"]]));
}
// F-16: a header error hides the version-mismatch and false-attestation alerts.
write("findings/f16-header-error-hides-alerts.csv", csv(tall("3.0.0", [{}],
  header("3.0.0", { last_updated_on: "July 1 2024", version: "2.0.0", [ATTESTATION]: "false" }))));
// F-17: the nine-9s alert compares with Number().
{
  const NINES = ["999999999", "999999999.0", "9.99999999e8", "+999999999", "0x3B9AC9FF"];
  for (const version of ["2.2.0", "3.0.0"]) {
    const key = v3(version) ? "median_amount" : "estimated_amount";
    write(`findings/f17-nine-nines-tall-${version}.csv`, csv(tall(version, NINES.map((n) => ({ [key]: n })))));
    write(`findings/f17-nine-nines-wide-${version}.csv`,
      csv(wide(version, ["Payer A | PPO"], NINES.map((n) => ({ [`${key} | Payer A | PPO`]: n })))));
  }
}
// F-20: fewer than four rows returns only MinRowsError, dropping the row-2 date error.
write("findings/f20-min-rows.csv", csv(tall("2.0.0", [], header("2.0.0", { last_updated_on: "July 1 2024" }))));
// F-23: false affirmation (2.x) and attestation (3.0) alerts anchored at row 3.
write("findings/f23-false-affirmation.csv", csv(tall("2.0.0", [{}], header("2.0.0", { [AFFIRMATION]: "false" }))));
write("findings/f23-false-attestation.csv", csv(tall("3.0.0", [{}], header("3.0.0", { [ATTESTATION]: "false" }))));
// F-24: a three-part column with one placeholder is neither reported nor matched (file stays
// valid); a four-part column with one placeholder is reported.
for (const [name, column] of [
  ["three-part-ignored", "additional_payer_notes | [payer_name] | Plan 1"],
  ["four-part-reported", "standard_charge | [payer_name] | Plan 2 | negotiated_dollar"],
]) {
  const rows = wide("3.0.0", ["Payer A | PPO"]);
  rows[2].push(column);
  rows[3].push("");
  write(`findings/f24-${name}.csv`, csv(rows));
}
// F-25: the license-number skip is case-sensitive.
write("findings/f25-license-upper.csv", csv(tall("2.0.0", [{}],
  header("2.0.0", { "license_number | CA": "" }, { "license_number | CA": "LICENSE_NUMBER | MD" }))));
write("findings/f25-license-lower.csv", csv(tall("2.0.0", [{}],
  header("2.0.0", { "license_number | CA": "" }, { "license_number | CA": "license_number | MD" }))));
// F-26: real CRLF and mixed line endings.
write("findings/f26-crlf.csv", csv(tall("2.0.0", [{}, {}])).replaceAll("\n", "\r\n"));
write("findings/f26-mixed.csv", csv(tall("2.0.0", [{}, {}])).split("\n").map((l, i) => (l && i % 2 ? l + "\r" : l)).join("\n"));
// F-27: payer names whose lower-then-upper round trip changes.
write("findings/f27-sharp-s.csv", csv(wide("2.0.0", ["Groẞ Health | PPO"])));
write("findings/f27-dotted-i.csv", csv(wide("2.0.0", ["İstanbul Care | PPO"])));
// F-28: malformed quotes are ignored; a stray quote merges fields, an unclosed one eats the file.
{
  const lines = csv(tall("2.0.0", [{}, {}, {}])).split("\n");
  const at = (i, text) => lines.map((l, j) => (j === i ? text : l)).join("\n");
  write("findings/f28-invalid-quotes.csv", at(4, '"Hip" replacement' + lines[4].slice("Hip replacement".length)));
  write("findings/f28-missing-quotes.csv", at(4, '"Hip replacement' + lines[4].slice("Hip replacement".length)));
}

// --- data-row rules: edges the fixtures and unit tests leave out ----------------------
{
  const GROUP = ["negotiated_dollar", "negotiated_percentage", "negotiated_algorithm", "methodology", "notes",
    "estimated_amount", "median_amount", "10th_percentile", "90th_percentile", "count"];
  // A charge group's column: tall's when p is undefined, else wide payer/plan p's.
  const col = (p, name) => {
    if (!GROUP.includes(name)) return name;
    if (name.startsWith("negotiated_") || name === "methodology") {
      return p ? `standard_charge | ${p} | ${name}` : `standard_charge | ${name}`;
    }
    if (name === "notes") return p ? `additional_payer_notes | ${p}` : "additional_generic_notes";
    return p ? `${name} | ${p}` : name;
  };
  const N = "999999999";
  const NINES = { estimated_amount: N, median_amount: N, "10th_percentile": N, "90th_percentile": N, count: N };
  // Each row: overrides for the shared columns and the first group, then for the second
  // group (wide only).
  const ROWS = [
    ...["10", "011", "00", "0", "1 THROUGH 10", "11", "19", "20", "100", "9", "12.0", "1e2", "+12", "１２"]
      .map((count) => [{ count }]),
    [{ negotiated_dollar: "", negotiated_percentage: "50" }],
    [{ negotiated_percentage: "50" }],
    [{ negotiated_dollar: "", negotiated_algorithm: "x", count: "0" }],
    [{ negotiated_percentage: "50", count: "00" }],
    [{ negotiated_percentage: "50", count: "0", notes: "n" }],
    [NINES, NINES],
    [{ methodology: "Other" }, { negotiated_dollar: "", negotiated_percentage: "5", methodology: "" }],
    [{ negotiated_dollar: "", methodology: "" }, { negotiated_dollar: "", methodology: "" }],
    [{ negotiated_dollar: "", methodology: "", "standard_charge | gross": "", "standard_charge | discounted_cash": "" },
      { negotiated_dollar: "", methodology: "" }],
    [{ "standard_charge | min": "" }],
    [{ "standard_charge | max": "" }, { negotiated_dollar: "" }],
    [{ payer_name: "", plan_name: "" }],
    [{ "code | 1 | type": "ndc" }],
    [{ "code | 1 | type": "CMG" }],
    [{ "code | 1": "", "code | 1 | type": "", modifiers: "25" }],
    [{ "code | 1": "", "code | 1 | type": "", modifiers: "25", negotiated_dollar: "", methodology: "" },
      { negotiated_dollar: "", methodology: "" }],
    [{ "code | 1": "", "code | 1 | type": "", modifiers: "25", negotiated_dollar: "", methodology: "", notes: "n" },
      { negotiated_dollar: "", methodology: "" }],
    [{ "code | 1": "", "code | 1 | type": "" }],
    [{ drug_unit_of_measurement: "0", drug_type_of_measurement: "ml" }],
    [{ drug_type_of_measurement: "GR" }],
    [{ setting: "BOTH", description: "" }],
  ];
  const PAYERS = ["Payer A | PPO", "Payer B | HMO"];
  const values = (row, payers) =>
    Object.fromEntries(row.slice(0, payers.length).flatMap((o, i) =>
      Object.entries(o).map(([k, v]) => [col(payers[i], k), v])));
  for (const version of ["2.0.0", "2.2.0", "3.0.0"]) {
    for (const [shape, rows] of [
      ["tall", tall(version, ROWS.map((r) => values(r, [undefined])))],
      ["wide", wide(version, PAYERS, ROWS.map((r) => values(r, PAYERS)))],
    ]) {
      // A blank line consumes a row number; missing cells read as empty, extra ones are ignored.
      const [first] = rows.slice(3);
      rows.push([""], first.slice(0, 3), [...first, "extra"]);
      write(`rules/${shape}-${version}.csv`, csv(rows));
    }
  }
}

// --- JSON ---------------------------------------------------------------------------

// F-21: a syntax error after invalid charge items returns only InvalidJsonError.
{
  const doc = fixture("sample-valid.json");
  for (const charge of doc.standard_charge_information) delete charge.description;
  const text = json(doc);
  const end = text.lastIndexOf("]");
  write("findings/f21-syntax-error-after-errors.json", text.slice(0, end) + "]]" + text.slice(end + 1));
}
// F-29: reaching the limit drops metadata errors and file-level alerts.
{
  const doc = fixture("sample-valid.json");
  delete doc.hospital_name;
  doc.version = "1.0.0";
  doc.affirmation.confirm_affirmation = false;
  for (const charge of doc.standard_charge_information) delete charge.description;
  write("findings/f29-limit-drops-metadata.json", json(doc));
}
// F-30: tokenizer errors carry a byte position within the current 64 KiB read chunk;
// token-order errors and end-of-input errors carry none.
{
  const doc = fixture("sample-valid.json");
  const text = json(doc);
  const version = '"version": "2.0.0",';
  write("findings/f30-parser-error-first-chunk.json", text.replace(version, version + ","));
  write("findings/f30-tokenizer-error-first-chunk.json", text.replace(version, '"version": @"2.0.0",'));
  write("findings/f30-tokenizer-error-non-ascii.json", text.replace(version, '"version": \u00e9"2.0.0",'));
  write("findings/f30-truncated-in-token.json", text.slice(0, text.indexOf(version) + 15));
  write("findings/f30-truncated-between-tokens.json", text.slice(0, text.indexOf(version) + version.length));
  doc.standard_charge_information[0].description = "x".repeat(70000);
  const big = json(doc);
  const end = big.lastIndexOf("]");
  write("findings/f30-parser-error-second-chunk.json", big.slice(0, end) + "]]" + big.slice(end + 1));
  write("findings/f30-tokenizer-error-second-chunk.json", big.slice(0, end) + "@]" + big.slice(end + 1));
}

// --- versions (F-22): validated with extra version strings by the oracle ---------------

write("versions/tall-2.0.0.csv", csv(tall("2.0.0")));
write("versions/sample-valid.json", json(fixture("sample-valid.json")));

// --- upstream unit tests ----------------------------------------------------------------
// target/lift.jsonl holds every direct call upstream's CSV unit tests make to a row-level
// method (lift.setup.mjs). Each becomes a whole file: the tested row in place, the other
// rows filled with valid defaults for the version. Validated at that version only.

const norm = (c) => (c ?? "").split("|").map((p) => p.trim()).join(" | ").toUpperCase();
function headerValues(cols, version) {
  const [defaultCols, defaultVals] = header(version);
  const byName = new Map(defaultCols.map((c, i) => [norm(c), defaultVals[i]]));
  return cols.map((c) => (norm(c).startsWith("LICENSE_NUMBER") ? "50056" : byName.get(norm(c)) ?? ""));
}

function lift({ method, arg, version, headerColumns, dataColumns, normalizedColumns }) {
  const [h, v] = header(version, { version: v3(version) ? "3.0.0" : "2.0.0" });
  const cols = tallColumns(version);
  switch (method) {
    case "validateHeaderColumns": {
      const row = arg.length ? arg : ["comment"]; // an empty row 1 would be a blank line
      return [row, headerValues(row, version), cols, item(cols)];
    }
    case "validateHeaderRow":
    case "alertHeaderRow":
      return [headerColumns, arg, cols, item(cols)];
    case "validateColumns":
    case "alertColumns":
      return [h, v, arg, []];
    case "validateDataRow":
    case "alertDataRow":
      return [h, v, dataColumns.map((c) => c ?? ""), dataColumns.map((_, i) => arg[normalizedColumns[i]] ?? "")];
  }
  throw new Error(`no lift for ${method}`);
}

if (fs.existsSync("target/lift.jsonl")) {
  const seen = new Map(); // file -> content
  for (const line of fs.readFileSync("target/lift.jsonl", "utf-8").trim().split("\n")) {
    const call = JSON.parse(line);
    const content = csv(lift(call));
    const slug = call.test.replace(/^CsvValidator v[\d.]+ > /, "").toLowerCase()
      .replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 100);
    const dir = `unit/v${call.version}`;
    if ([...seen].some(([f, c]) => f.startsWith(dir) && c === content)) continue;
    let file = `${dir}/${slug}.csv`;
    for (let n = 2; seen.has(file); n++) file = `${dir}/${slug}-${n}.csv`;
    seen.set(file, content);
    write(file, content);
  }
}
