// Writes tests/cases/findings: one or more input files per upstream quirk the port
// reproduces, tests/cases/rules for data-row rule edges, tests/cases/json
// for JSON parser and schema edges, and tests/cases/versions for version-string handling.
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

// A blank row 1 (no BOM) fails; the same file with a BOM is valid, one row lower.
write("findings/blank-row1.csv", "\n" + csv(tall("2.0.0")));
write("findings/bom-blank-row1.csv", "﻿\n" + csv(tall("2.0.0")));
// A blank row 3 returns only HeaderBlankError, dropping the row-2 date error.
{
  const [h, v, ...rest] = tall("2.0.0", [{}], header("2.0.0", { last_updated_on: "July 1 2024" }));
  write("findings/blank-row3.csv", csv([h, v]) + "\n" + csv(rest.slice(1)));
}
// A data row of whitespace only is blank.
{
  const rows = tall("2.0.0", [{}, {}]);
  rows.splice(4, 0, [" ", "\t", " "]);
  write("findings/whitespace-row.csv", csv(rows));
}
// A CSV declaring 2.2.0 gets VersionMismatchAlert anchored at row 3; JSON does not.
write("findings/version-2.2.0.csv", csv(tall("2.2.0", [{}], header("2.2.0"))));
write("findings/version-2.2.0.json", json({ ...fixture("sample-valid.json"), version: "2.2.0" }));
// Years below 100.
write("findings/slash-year-50.csv", csv(tall("2.0.0", [{}], header("2.0.0", { last_updated_on: "1/1/0050" }))));
write("findings/iso-year-50.csv", csv(tall("2.0.0", [{}], header("2.0.0", { last_updated_on: "0050-01-01" }))));
// A tall modifier row without information names additional_payer_notes.
write("findings/modifier-tall.csv", csv(tall("3.0.0", [{}, {
  "code | 1": "", "code | 1 | type": "", modifiers: "50", payer_name: "", plan_name: "",
  "standard_charge | negotiated_dollar": "", "standard_charge | methodology": "", "standard_charge | min": "",
  "standard_charge | max": "", "standard_charge | gross": "", "standard_charge | discounted_cash": "",
}])));
// More missing columns than the limit, then ProblemsInHeaderError on top.
{
  const [h, v] = header("2.0.0");
  write("findings/many-missing-columns.csv", csv([h, v, ["description", "payer_name", "plan_name"], ["x", "a", "b"]]));
}
// A header error hides the version-mismatch and false-attestation alerts.
write("findings/header-error-hides-alerts.csv", csv(tall("3.0.0", [{}],
  header("3.0.0", { last_updated_on: "July 1 2024", version: "2.0.0", [ATTESTATION]: "false" }))));
// The nine-9s alert compares with Number().
{
  const NINES = ["999999999", "999999999.0", "9.99999999e8", "+999999999", "0x3B9AC9FF"];
  for (const version of ["2.2.0", "3.0.0"]) {
    const key = v3(version) ? "median_amount" : "estimated_amount";
    write(`findings/nine-nines-tall-${version}.csv`, csv(tall(version, NINES.map((n) => ({ [key]: n })))));
    write(`findings/nine-nines-wide-${version}.csv`,
      csv(wide(version, ["Payer A | PPO"], NINES.map((n) => ({ [`${key} | Payer A | PPO`]: n })))));
  }
}
// Fewer than four rows returns only MinRowsError, dropping the row-2 date error.
write("findings/min-rows.csv", csv(tall("2.0.0", [], header("2.0.0", { last_updated_on: "July 1 2024" }))));
// False affirmation (2.x) and attestation (3.0) alerts anchored at row 3.
write("findings/false-affirmation.csv", csv(tall("2.0.0", [{}], header("2.0.0", { [AFFIRMATION]: "false" }))));
write("findings/false-attestation.csv", csv(tall("3.0.0", [{}], header("3.0.0", { [ATTESTATION]: "false" }))));
// A three-part column with one placeholder is neither reported nor matched (file stays
// valid); a four-part column with one placeholder is reported.
for (const [name, column] of [
  ["three-part-ignored", "additional_payer_notes | [payer_name] | Plan 1"],
  ["four-part-reported", "standard_charge | [payer_name] | Plan 2 | negotiated_dollar"],
]) {
  const rows = wide("3.0.0", ["Payer A | PPO"]);
  rows[2].push(column);
  rows[3].push("");
  write(`findings/placeholder-${name}.csv`, csv(rows));
}
// The license-number skip is case-sensitive.
write("findings/license-upper.csv", csv(tall("2.0.0", [{}],
  header("2.0.0", { "license_number | CA": "" }, { "license_number | CA": "LICENSE_NUMBER | MD" }))));
write("findings/license-lower.csv", csv(tall("2.0.0", [{}],
  header("2.0.0", { "license_number | CA": "" }, { "license_number | CA": "license_number | MD" }))));
// ... and its two characters after the bar exclude line terminators (\uFB02 upper-cases to FL).
write("findings/license-newline-ligature.csv", csv(tall("2.0.0", [{}],
  header("2.0.0", { "license_number | CA": "" }, { "license_number | CA": "license_number |\n\uFB02" }))));
// A second license column: an invalid state before the matching one is reported, one after
// it is neither reported nor a duplicate.
for (const [name, first, second] of [
  ["invalid-first", "license_number | ZZ", "license_number | CA"],
  ["invalid-second", "license_number | CA", "license_number | ZZ"],
]) {
  const [h, v, ...rest] = tall("2.0.0", [{}], header("2.0.0", {}, { "license_number | CA": first }));
  write(`findings/license-twice-${name}.csv`, csv([[...h, second], [...v, "50057"], ...rest]));
}
// Only `code | N` and `code | N | type` count as code columns, and a four-part column is a
// charge column only with a known charge.
{
  const rows = tall("2.0.0");
  rows[2].push("notes | 2", "notes | 3 | type");
  rows[3].push("", "");
  write("findings/code-like-columns.csv", csv(rows));
}
{
  const rows = wide("3.0.0", ["Payer A | PPO"]);
  rows[2].push("standard_charge | Payer Z | Plan Z | bogus");
  rows[3].push("");
  write("findings/four-part-unknown-charge.csv", csv(rows));
}
// A code count past row 3's columns: the pairs missing from it fill any error limit.
{
  const rows = tall("2.0.0");
  rows[2].push("code | 3 | type", "code | 40");
  rows[3].push("", "");
  write("findings/code-count-past-columns.csv", csv(rows));
}
// Real CRLF and mixed line endings.
write("findings/crlf.csv", csv(tall("2.0.0", [{}, {}])).replaceAll("\n", "\r\n"));
write("findings/mixed-line-endings.csv", csv(tall("2.0.0", [{}, {}])).split("\n").map((l, i) => (l && i % 2 ? l + "\r" : l)).join("\n"));
// Payer names whose lower-then-upper round trip changes.
write("findings/sharp-s.csv", csv(wide("2.0.0", ["Groẞ Health | PPO"])));
write("findings/dotted-i.csv", csv(wide("2.0.0", ["İstanbul Care | PPO"])));
// A message with ', " and ` is shown in single quotes; control characters are escaped.
write("findings/quotes-and-controls.csv", csv(tall("2.0.0", [{ setting: "a'b`c\u0001\u007f" }])));
// Malformed quotes are ignored; a stray quote merges fields, an unclosed one eats the file.
{
  const lines = csv(tall("2.0.0", [{}, {}, {}])).split("\n");
  const at = (i, text) => lines.map((l, j) => (j === i ? text : l)).join("\n");
  write("findings/invalid-quotes.csv", at(4, '"Hip" replacement' + lines[4].slice("Hip replacement".length)));
  write("findings/missing-quotes.csv", at(4, '"Hip replacement' + lines[4].slice("Hip replacement".length)));
  write("findings/bom-invalid-quotes.csv", "\ufeff\n" + at(4, '"Hip" replacement' + lines[4].slice("Hip replacement".length)));
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
    // A wide group with nothing but a count of zero still needs notes.
    [{}, { negotiated_dollar: "", methodology: "", count: "0" }],
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
    // Dotless i upper-cases to I, so this is an allowed setting.
    [{ setting: "ınpatient" }],
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
      const first = rows[3];
      rows.push([""], first.slice(0, 3), [...first, "extra"]);
      write(`rules/${shape}-${version}.csv`, csv(rows));
    }
  }
  // Row 3 may interleave the wide groups' columns; the groups keep their order.
  const KINDS = ["negotiated_dollar", "negotiated_percentage", "negotiated_algorithm", "methodology"];
  const cols = [...BASE, ...drug("3.0.0"), ...AMOUNTS,
    ...KINDS.flatMap((k) => PAYERS.map((p) => `standard_charge | ${p} | ${k}`)),
    ...PAYERS.flatMap((p) => [`additional_payer_notes | ${p}`, ...extra("3.0.0", ` | ${p}`)]), ...TAIL];
  const row = item(cols, values([{ negotiated_dollar: "", negotiated_percentage: "x" }, { negotiated_dollar: "-5" }],
    PAYERS));
  write("rules/wide-interleaved-3.0.0.csv", csv([...header("3.0.0"), cols, row]));
}

// --- JSON ---------------------------------------------------------------------------

// A syntax error after invalid charge items returns only InvalidJsonError.
{
  const doc = fixture("sample-valid.json");
  for (const charge of doc.standard_charge_information) delete charge.description;
  const text = json(doc);
  const end = text.lastIndexOf("]");
  write("findings/syntax-error-after-errors.json", text.slice(0, end) + "]]" + text.slice(end + 1));
}
// Reaching the limit drops metadata errors and file-level alerts.
{
  const doc = fixture("sample-valid.json");
  delete doc.hospital_name;
  doc.version = "1.0.0";
  doc.affirmation.confirm_affirmation = false;
  for (const charge of doc.standard_charge_information) delete charge.description;
  write("findings/limit-drops-metadata.json", json(doc));
}
// Tokenizer errors carry a byte position within the current 64 KiB read chunk;
// token-order errors and end-of-input errors carry none.
{
  const doc = fixture("sample-valid.json");
  const text = json(doc);
  const version = '"version": "2.0.0",';
  write("findings/parser-error-first-chunk.json", text.replace(version, version + ","));
  write("findings/tokenizer-error-first-chunk.json", text.replace(version, '"version": @"2.0.0",'));
  write("findings/tokenizer-error-non-ascii.json", text.replace(version, '"version": \u00e9"2.0.0",'));
  write("findings/truncated-in-token.json", text.slice(0, text.indexOf(version) + 15));
  write("findings/truncated-between-tokens.json", text.slice(0, text.indexOf(version) + version.length));
  doc.standard_charge_information[0].description = "x".repeat(70000);
  const big = json(doc);
  const end = big.lastIndexOf("]");
  write("findings/parser-error-second-chunk.json", big.slice(0, end) + "]]" + big.slice(end + 1));
  write("findings/tokenizer-error-second-chunk.json", big.slice(0, end) + "@]" + big.slice(end + 1));
}

// --- JSON parser and schema edges: what the fixtures leave out --------------------------
{
  // Tokenizer, token parser and end-of-input paths, each a tiny document.
  const u = (hex) => "\\u" + hex; // a JSON \u escape, built from parts: some tools decode a literal one
  const SYNTAX = {
    "root-trailing-comma": String.raw`{"hospital_name": "x", "version": "1.0.0",}`,
    "item-trailing-comma": String.raw`{"standard_charge_information": [{"description": "x",}]}`,
    "unkept-trailing-comma": String.raw`{"other": {"a": 1, "b": [2],}, "version": "1.0.0"}`,
    "array-trailing-comma": String.raw`{"standard_charge_information": [{"description": "x"},]}`,
    "unkept-array-trailing-comma": String.raw`{"other": [1,], "version": "1.0.0"}`,
    "empty-containers": String.raw`{"other": {}, "standard_charge_information": [], "version": "1.0.0"}`,
    "bom-or-start": String.raw`{@`,
    "bom-or-start-nested": String.raw`[[[ ]],@`,
    "number-token": String.raw`{"a" 1.50}`,
    "number-token-huge": String.raw`{"a" 1e400}`,
    "number-token-negative-zero": String.raw`{"a" -0}`,
    "number-token-exponent": String.raw`{"a" 1e21}`,
    "number-token-small": String.raw`{"a" 0.0000001}`,
    "true-token": String.raw`{"a" true}`,
    "false-token": String.raw`{"a" false}`,
    "null-token": String.raw`{"a" null}`,
    "string-token": String.raw`{"a" "q\"\\\n` + u("0001") + u("00e9") + String.raw`\/"}`,
    "brace-token": String.raw`{"a" {`,
    "bracket-token": String.raw`{"a" [`,
    "colon-token": String.raw`{:`,
    "comma-token": String.raw`[,`,
    "right-brace-in-value": String.raw`{"a": }`,
    "brace-closes-array": "[1}",
    "bracket-closes-object": String.raw`{"a": 1]`,
    "leading-zero": String.raw`{"a": 01}`,
    "full-stop": String.raw`{"a": 1.}`,
    "minus": String.raw`{"a": -}`,
    "exponent": String.raw`{"a": 1e}`,
    "exponent-sign": String.raw`{"a": 1e+}`,
    "exponent-sign-twice": String.raw`{"a": 1e+-1}`,
    "decimal-dot": String.raw`{"a": 1.5.2}`,
    "true-typo": String.raw`{"a": trux}`,
    "false-typo": String.raw`{"a": falsy}`,
    "null-typo": String.raw`{"a": nulx}`,
    "bad-escape": String.raw`{"version": "\x"}`,
    "bad-unicode": String.raw`{"version": "\u12g4"}`,
    "control-in-string": '{"version": "a\tb"}',
    "surrogates": `{"version": "${u("d83d")}${u("de00")} ${u("d800")}${u("0041")}z ${u("dc00")} ${u("d800")}Az ${u("dc00")} ${u("00e9")} ${u("D83D")}${u("DE00")}"}`,
    "surrogate-across-strings": String.raw`{"version": "\ud800", "a" "\udc00"}`,
    "escapes": String.raw`{"version": "\b\f\n\r\t\/\\\""}`,
    "root-string": '"abc"',
    "root-number": "123",
    "root-number-newline": "123\n",
    "root-minus": "-",
    "root-array": String.raw`[{"hospital_name": "x"}]`,
    "root-null": "null",
    "root-true": "true",
    "empty": "",
    "whitespace": " \n\t\r ",
    "garbage-after-root": String.raw`{"version": "1.0.0"} xyz`,
    "two-roots": String.raw`{"version": "1.0.0"}{"version": "2.0.0"}`,
    "duplicate-metadata": String.raw`{"version": "1.0.0", "version": "9.9.9"}`,
    "charges-object": String.raw`{"standard_charge_information": {"a": {"description": "x"}, "version": "8.8.8"}}`,
    "charges-primitives": String.raw`{"standard_charge_information": [1, null, "x", [], true, 1e400]}`,
    "charges-twice": String.raw`{"standard_charge_information": [{}], "standard_charge_information": [{}, {}]}`,
    "charges-nested-name": String.raw`{"standard_charge_information": {"standard_charge_information": 5}}`,
    "charges-deep": String.raw`{"x": {"y": {"standard_charge_information": [{}]}}}`,
    "duplicate-item-member": String.raw`{"standard_charge_information": [{"description": 5, "description": "x"}]}`,
    "truncated-number": String.raw`{"a": 12`,
    "truncated-item-number": String.raw`{"standard_charge_information": [12`,
    "truncated-string": String.raw`{"a": "x`,
    "truncated-escape": '{"a": "\\',
    "truncated-unicode": String.raw`{"a": "\u12`,
    "truncated-true": String.raw`{"a": tr`,
    "truncated-value": String.raw`{"a":`,
    "truncated-colon": String.raw`{"a"`,
    "truncated-array": "[1",
    "truncated-nested": String.raw`{"a": [{"b": 1}`,
    "bom-whitespace-error": "﻿ \n\n{@",
    "bom-twice": "﻿﻿{}",
    "bom-only": "﻿",
    "bom-not-first": " ﻿{}",
    "number-two-exponents": '{"a": 1e5e5}',
  };
  for (const [name, text] of Object.entries(SYNTAX)) write(`json/syntax/${name}.json`, text);

  // Bytes Node's decoder replaces or carries across the 64 KiB read boundary.
  const bytes = (...parts) => Buffer.concat(parts.map((p) => Buffer.from(p)));
  write("json/syntax/invalid-byte-at-end.json", bytes('{"a": 1 ', [0xe2, 0x82]));
  write("json/syntax/invalid-byte-in-string.json", bytes('{"version": "a', [0xff], 'b"}'));
  write("json/syntax/invalid-byte-shifts-position.json", bytes('{"version": "', [0xff], '", @'));
  const pad = (n) => '{"version": "' + "x".repeat(n - 13);
  write("json/syntax/split-char-second-chunk.json", bytes(pad(65535), "é", '", @'));
  write("json/syntax/invalid-byte-second-chunk.json", bytes(pad(65535), [0xc3], '", @'));
  write("json/syntax/bom-second-chunk.json", bytes(pad(65536), [0xef, 0xbb, 0xbf], '", @'));

  // Schema keywords on the metadata and on charge items.
  const v2doc = () => fixture("sample-valid.json");
  const v3doc = () => fixture("3.0/sample-valid.json");
  const INF = "__INFINITY__"; // 1e400 in the file, which JSON.stringify cannot write
  const raw = (doc) => json(doc).replaceAll(`"${INF}"`, "1e400");
  {
    const doc = v3doc();
    Object.assign(doc, {
      hospital_name: "",
      location_name: [],
      hospital_address: ["", 5],
      license_information: { state: "XX", license_number: "" },
      type_2_npi: "1234567890",
      attestation: { attestation: "no", confirm_attestation: "yes", attester_name: "" },
      version: 3,
      modifier_information: [{ description: "", code: 5, modifier_payer_information: [], setting: "x" }, 5],
      affirmation: { affirmation: "no", confirm_affirmation: 1 },
      hospital_location: [""],
    });
    write("json/schema/metadata.json", json(doc));
    write("json/schema/metadata-wrong-types.json", raw({
      hospital_name: 5, last_updated_on: null, location_name: {}, hospital_address: "x", license_information: [],
      type_2_npi: [""], attestation: "yes", version: "", modifier_information: {}, affirmation: [],
      hospital_location: INF, standard_charge_information: [{}],
    }));
  }
  ["2024-02-29", "2023-02-29", "1900-02-29", "2000-02-29", "2024-04-31", "2024-13-01", "2024-00-01", "2024-01-00",
    "2024-1-01", "24-01-01", "2024-01-01T00:00:00Z", "２０２４-01-01", "2024/01/01", "", 20240101, "2024-01-01\n"]
    .forEach((date, i) => write(`json/schema/date-${i}.json`, json({ last_updated_on: date })));
  {
    const doc = v3doc();
    const payer = doc.standard_charge_information[0].standard_charges[0].payers_information[0];
    const COUNTS = ["0", "1 through 10", "1 THROUGH 10", "10", "011", "11", "19", "20", "100", "9", "abc", "", 5, "１２",
      "12 ", "00", "999999999"];
    doc.standard_charge_information[0].standard_charges[0].payers_information = COUNTS.map((count) => ({
      ...payer, standard_charge_dollar: undefined, standard_charge_percentage: 50, count,
    }));
    write("json/schema/counts.json", json(doc));
  }
  {
    const doc = v3doc();
    const item = doc.standard_charge_information[0];
    const charge = item.standard_charges[0];
    const payer = charge.payers_information[0];
    doc.standard_charge_information = [
      5, "x", null, [], true, {},
      { description: 5, code_information: [], standard_charges: [] },
      { description: "x", code_information: [{ code: "", type: "BAD" }, { type: "NDC", code: "1" }, 5], standard_charges: [5, {}] },
      { ...item, drug_information: { unit: 0, type: "XX" } },
      { ...item, drug_information: { unit: "1" } },
      { ...item, drug_information: 5 },
      { ...item, code_information: [{ code: "1", type: "NDC" }] },
      { ...item, code_information: undefined },
      { ...item, standard_charges: [{ ...charge, setting: "x", minimum: 0, maximum: -5, gross_charge: "1",
        discounted_cash: INF, modifier_code: [""], additional_generic_notes: "" }] },
      { ...item, standard_charges: [{ setting: "both", modifier_code: [] }] },
      { ...item, standard_charges: [{ setting: "both", payers_information: [] }] },
      { ...item, standard_charges: [{ setting: "both", payers_information: [5, {}] }] },
      { ...item, standard_charges: [{ setting: "both", payers_information: [
        { payer_name: "", plan_name: 5, standard_charge_percentage: 0, methodology: "other" },
        { payer_name: "a", plan_name: "b", standard_charge_dollar: 1, methodology: "case rate" },
        { payer_name: "a", plan_name: "b", standard_charge_algorithm: "", methodology: "per diem", count: "0",
          median_amount: 0, "10th_percentile": -1, "90th_percentile": "1", estimated_amount: 5 },
        { ...payer, methodology: "fee schedule", additional_payer_notes: "" },
      ] }] },
    ];
    write("json/schema/charge-items.json", raw(doc));
  }
  {
    // Nine 9s and estimated amounts, for both alert schemas; more than two, for the limit.
    const N = 999999999;
    for (const [name, doc] of [["v2", v2doc()], ["v3", v3doc()]]) {
      const payer = doc.standard_charge_information[0].standard_charges[0].payers_information[0];
      doc.standard_charge_information[0].standard_charges[0].payers_information = [
        { ...payer, estimated_amount: N, count: String(N), median_amount: N, "10th_percentile": N, "90th_percentile": N },
        { ...payer, estimated_amount: String(N), count: N, median_amount: "__NINES_DECIMAL__" },
        { ...payer, estimated_amount: "__NINES_EXPONENT__" },
      ];
      write(`json/schema/nines-${name}.json`, json(doc)
        .replace('"__NINES_DECIMAL__"', "999999999.0").replace('"__NINES_EXPONENT__"', "9.99999999e8"));
    }
  }
}

// --- versions: validated with extra version strings by the oracle ---------------

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
