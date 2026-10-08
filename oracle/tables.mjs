// Writes the JavaScript behaviour the port re-implements:
//   tests/js.json        trim set, case maps, Number(), and upstream's own helpers on edge inputs
//   tests/reader/*.csv   the reader corpus; *.json next to each holds papaparse's rows per chunk size
import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
import { pathToFileURL } from "node:url";

const upstream = path.resolve(process.argv[2]);
const lib = (file) => import(pathToFileURL(path.join(upstream, "lib", file)));
const require = createRequire(path.join(upstream, "package.json"));
const semver = require("semver");
const Papa = require("papaparse");
const { isValidDate, dynaValidateOptionalFloatField } = await lib("validators/CsvHelpers.js");
const { validateFilename } = await lib("filename.js");
const { removeBOM } = await lib("utils.js");

// --- JS semantics -------------------------------------------------------------

const chars = [];
for (let cp = 0; cp <= 0x10ffff; cp++) {
  if (cp < 0xd800 || cp > 0xdfff) chars.push([cp, String.fromCodePoint(cp)]);
}
const trim = chars.filter(([, s]) => s.trim() === "").map(([cp]) => cp);
for (const [cp, s] of chars) {
  if (/^\s$/.test(s) !== trim.includes(cp)) throw new Error(`\\s and trim() differ at ${cp}`);
  if (s.toLocaleUpperCase() !== s.toUpperCase()) throw new Error(`locale upper case differs at ${cp}`);
  if (s.toLocaleLowerCase() !== s.toLowerCase()) throw new Error(`locale lower case differs at ${cp}`);
}
const caseMap = (f) => Object.fromEntries(chars.filter(([, s]) => f(s) !== s).map(([cp, s]) => [cp, f(s)]));
const upper = caseMap((s) => s.toUpperCase());
const lower = caseMap((s) => s.toLowerCase());
// Lower-casing a string is not per character: a final capital sigma becomes ς.
const LOWER = ["Σ", "ΑΣ", "ΑΣ Β", "ΑΣ.Β", "Α.Σ", "ΑΣ'", "ΑΣ'Β", "ΑΣ\u00adΒ", "Σ Α", "Payer Σ | PLAN ΑΣ", "1Σ", "ΑΣ1",
  "ΑΣ\u{16ea0}", "ΑΣ\u{16ebb}Β", "\u{16ebb}Σ", "ΑΣ\ua7d3"]; // Unicode 17 letters are unassigned to Node 20

// Non-finite numbers and -0 do not survive JSON, so they are written as strings.
const num = (n) => (Number.isFinite(n) && !Object.is(n, -0) ? n : String(Object.is(n, -0) ? "-0" : n));
const NINES = ["999999999", " 999999999 ", " 999999999﻿", " 999999999", "999999999\u0085",
  "999999999.0", "999999999.00000000001", "9.99999999e8", "9.99999999E+8", "+999999999", "-999999999",
  "0999999999", "0x3B9AC9FF", "0X3b9ac9ff", "0o7346544777", "0b111011100110101100100111111111",
  "999_999_999", "999,999,999", "999999999a", "٩٩٩", "９９９"];
const NUMBERS = [...NINES, "", " ", "\t\n", "0", "-0", ".5", "5.", ".", "+", "-", "1e1000", "-1e1000", "1e-400",
  "Infinity", "-Infinity", "+Infinity", "infinity", "inf", "NaN", "0x", "0x1g", "0b2", "0o8", "-0x10", "1__0",
  "12345678901234567890", "123456789012345678901234567890", "0.1", "00", "1 2"];

// String(n), which JSON parser errors and Ajv messages print numbers with.
const STRINGS = [...NUMBERS, "1e21", "1e20", "123e19", "123e18", "1e-6", "1e-7", "1.5e-7", "0.000001234", "5e-324",
  "1.7976931348623157e308", "4.35", "-1.5", "1e100", "1.25e-5", "0.1e1", "100", "2.5e-300"];

const DATES = ["2024-01-31", "2024-02-29", "2023-02-29", "2024-13-01", "2024-00-10", "2024-01-00", "2024-1-01",
  "0000-01-01", "0099-12-31", "9999-12-31", "2024-01-01\n", " 2024-01-01", "2024-01-01 ", "٢٠٢٤-01-01",
  "1/2/2024", "01/02/2024", "12/31/0099", "1/1/0000", "1/1/0100", "2/29/2000", "2/29/1900", "2/30/2024",
  "13/1/2024", "0/1/2024", "1/0/2024", "001/1/2024", "1/1/24", "1/1/2024\n", "1-1-2024", "2024/01/01", ""];

const POSITIVE = ["1", "0", "0.0", "00", "0.", ".0", ".5", "5.", "1.5", "01", "1e5", "-1", "+1", " 1", "1 ",
  "1,000", "1.2.3", ".", "Infinity", "0x10", "٣", "１", "0." + "0".repeat(400) + "1", "9".repeat(400)];

const COERCE = ["v2.0.0", "2.0.0", "2.0", "2", "3.0.0", "3.0", "3", "v3", "V3.0.0", " 3.0.0 ", "=3.0.0", "3.0.0-beta",
  "3.0.0+build", "3.0.0.0", "version3.0.0", "x.y.z", "", "3.1", "2.2.1", "03.0.0", "3.00.0", "3.0.0x",
  "a3b", "3.0.0 2.0.0", "2.0.0.3.0.0", "99999999999999999.0.0", "3.١.0", "9007199254740992", "3-4"];

const FILENAMES = ["123456789_hospital_standardcharges.json", "12-3456789_x_standardcharges.csv",
  "123456789-1234567890_x_standardcharges.csv", "123456789_x_standardcharges.CSV", "123456789_x_STANDARDCHARGES.Json",
  "123456789__standardcharges.json", "123456789_a_b_standardcharges.json", "12345678_x_standardcharges.json",
  "1234567890_x_standardcharges.json", "123456789_x_standardcharges.json\n", "123456789_x_standardcharges.json.gz",
  "123456789_\n_standardcharges.json", "123456789_x y_standardcharges.json", "123456789_é_standardcharges.json",
  "123456789_x_ſtandardcharges.json", "١٢٣456789_x_standardcharges.json", "123456789_x_standardcharges.txt"];

const positive = (value) => dynaValidateOptionalFloatField(["f"], ["f"], "f", { f: value }, 1).length === 0;
const coerce = (v) => semver.coerce(v)?.toString() ?? v; // CsvValidator and JsonValidator constructors

fs.writeFileSync("tests/js.json", JSON.stringify({
  trim,
  upper,
  lower,
  lowerStrings: LOWER.map((s) => [s, s.toLocaleLowerCase()]),
  number: NUMBERS.map((s) => [s, num(Number(s))]),
  numberString: STRINGS.map((s) => [s, String(Number(s))]),
  isValidDate: DATES.map((s) => [s, isValidDate(s)]),
  positive: POSITIVE.map((s) => [s, positive(s)]),
  coerce: COERCE.map((s) => [s, coerce(s)]),
  validateFilename: FILENAMES.map((s) => [s, validateFilename(s)]),
}, null, 1) + "\n");

// --- reader corpus --------------------------------------------------------------

const b = (s) => Buffer.from(s, "utf-8");
const CORPUS = {
  "empty": b(""),
  "newline": b("\n"),
  "newlines": b("\n\n\n"),
  "trailing-newline": b("a,b\n"),
  "no-trailing-newline": b("a,b\nc,d"),
  "blank-lines-between": b("a,b\n\n \nc,d\n\n"),
  "crlf": b("a,b\r\nc,d\r\n"),
  "cr-only": b("a,b\rc,d\r"),
  "mixed-endings": b("a,b\r\nc,d\ne,f\rg\n"),
  "ragged": b("a,b,c\nd\ne,f,g,h\n,\n,,\n"),
  "spaces": b(" a , b \n\t c ,　d\n"),
  "quoted": b('"a","b,c","d\ne","f""g"\n'),
  "quoted-crlf-inside": b('"a\r\nb",c\r\n'),
  "quoted-empty": b('"",""\n""\n'),
  "quote-mid-field": b('a"b,c"d\n'),
  "quote-after-space": b(' "a",b\n'),
  "quote-then-text": b('"a"b,c\n'),
  "quote-then-spaces": b('"a" ,b\n"c"\t,d\n"e"  \n'),
  "quote-then-nbsp": b('"a" ,b\n"c" ,d\n'),
  "quote-then-cr": b('"a"\r\n"b"\r'),
  "quote-unclosed": b('a,"b\nc,d\n'),
  "quote-unclosed-eof": b('a,"b'),
  "quote-closed-eof": b('a,"b"'),
  "quoted-doubled-twice": b('"a""b","c""d"\n'),
  "quote-doubled-unclosed": b('"a""\nb\n'),
  "quote-only": b('"\n'),
  "quotes-many-bad": b('"a"x"b"y,c\n"d"e\n'),
  "bom": b("﻿a,b\nc,d\n"),
  "bom-blank-lines": b("﻿\n\n\na,b\n"),
  "bom-spaces": b("﻿ \t a,b\n"),
  "bom-twice": b("﻿﻿a,b\n"),
  "bom-not-first": b("a,b\n﻿c,d\n"),
  "bom-only": b("﻿"),
  "no-bom-blank-lines": b("\n\na,b\n"),
  // Upstream reads 64 KiB chunks; only the first goes through removeBOM's trimStart.
  "bom-whitespace-past-64k": b("\ufeff\n\n\n" + " ".repeat(70000) + "\n\na,b\n"),
  // 1024-byte chunks split these; 1-byte chunks split every input everywhere.
  "quoted-across-chunk": b('"' + "x".repeat(1100) + '",b\nc,d\n'),
  "field-across-chunk": b("a".repeat(1100) + ",b\nc,d\n"),
  // A quoted field longer than 128 bytes, with a char across its 128th byte.
  "quoted-long-utf8": b('"' + "x".repeat(127) + "\u00e9" + "y".repeat(10) + '",b\n'),
  "utf8-split-at-chunk": b("a".repeat(1023) + "\u00e9,b\n"),
  "utf8-4byte-split-at-chunk": b("a".repeat(1022) + "\u{1f600},b\n"),
  "invalid-utf8": Buffer.from([0x61, 0xff, 0x2c, 0x62, 0x0a, 0xc3, 0x28, 0x0a, 0xe2, 0x82, 0x2c, 0xc0, 0xaf, 0x0a]),
  "invalid-utf8-surrogate": Buffer.from([0xed, 0xa0, 0x80, 0x2c, 0x61, 0x0a]),
  "invalid-utf8-truncated-eof": Buffer.from([0x61, 0x2c, 0xe2, 0x82]),
  // An invalid byte, then a char cut by the 1024-byte chunk.
  "invalid-utf8-then-split": Buffer.concat([b("a".repeat(1022)), Buffer.from([0xff, 0xc3, 0xa9]), b(",b\n")]),
  "nul-and-controls": b("a\u0000,b\u0001\n\u000b,\u000c\n"),
  "semicolon-tab": b("a;b\tc\n"),
};

const parse = (file, highWaterMark) => new Promise((resolve, reject) => {
  const rows = [];
  const errors = [];
  Papa.parse(fs.createReadStream(file, { encoding: "utf-8", highWaterMark }), {
    header: false, newline: "\n", delimiter: ",", // CsvValidator.validate
    beforeFirstChunk: (chunk) => removeBOM(chunk),
    step: (step) => {
      rows.push(step.data);
      for (const e of step.errors) errors.push([rows.length - 1, e.code]);
    },
    complete: () => resolve({ rows, errors }),
    error: reject,
  });
});

fs.mkdirSync("tests/reader", { recursive: true });
let chunkSensitive = 0;
for (const [name, bytes] of Object.entries(CORPUS)) {
  const file = path.join("tests/reader", name + ".csv");
  fs.writeFileSync(file, bytes);
  const golden = {};
  for (const size of [1, 1024, 65536]) golden[size] = await parse(file, size);
  if (new Set(Object.values(golden).map((g) => JSON.stringify(g))).size > 1) chunkSensitive++;
  fs.writeFileSync(path.join("tests/reader", name + ".json"), JSON.stringify(golden) + "\n");
}
console.log(`js tables: ${trim.length} trim, ${Object.keys(upper).length} upper, ${Object.keys(lower).length} lower; ` +
  `reader: ${Object.keys(CORPUS).length} inputs, ${chunkSensitive} chunk-size sensitive`);
