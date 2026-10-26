---
name: hpt-validator
description: Calling the hpt-validator crate from Rust to validate CMS Hospital Price Transparency files (CSV tall or wide, JSON). Use when writing or changing code that calls hpt_validator::validate or reads its Report, Finding or Code.
---

# Using the hpt-validator crate

`hpt_validator::validate` checks a CMS Hospital Price Transparency machine-readable file
against a data dictionary version and returns upstream's errors and alerts
(`@cmsgov/hpt-validator` 2.6.1), byte for byte in Default mode.

```rust
use hpt_validator::{Format, Mode, Options, validate};

let options = Options { max_errors: 1000, mode: Mode::Default };
let report = validate(reader, Format::Csv, "3.0.0", &options)?;
if !report.valid() {
    for f in &report.errors {
        println!("{} {}", f.code.as_str(), f.to_json());
    }
}
```

## Input

- `reader` is any `std::io::Read`: a file, stdin, a decompressor such as
  `flate2::read::GzDecoder`. The crate streams; never read the file into memory first.
- The format is the caller's choice (`Format::Csv` or `Format::Json`); upstream takes it
  from the extension.
- The version is coerced as upstream does: `"v3"`, `"3.0"` and `"3.0.0"` are the same.
  Known: 2.0.0, 2.1.0, 2.2.0, 3.0.0.
- `validate_filename(name)` checks the CMS naming convention
  (`<EIN>[-<NPI>]_<hospital name>_standardcharges.csv|json`); it is separate from
  `validate`.

## Options

- `max_errors`: upstream's `maxErrors`. Validation stops after that many errors; alerts
  and Extended findings are capped at the same number separately. `0` is unlimited.
  `Options::default()` is unlimited; upstream's CLI uses 1000.
- `mode`: `Mode::Default` is exactly upstream's result. `Mode::Extended` adds
  `Report::file_type`, `Report::extended` and `Finding::line`, and changes nothing else.

## Result

- `Err` only when no report can be made: `Error::Io` (reading failed) or
  `Error::UnknownJsonVersion` (upstream throws for it). Everything wrong with the file
  itself is a finding. An unknown CSV version is the finding `HPT_CSV_INVALID_VERSION`,
  not an `Err`.
- `Report::valid()` is `errors.is_empty()`. Alerts never make a file invalid.
- `errors` and `alerts` are in upstream's order; keep it.

## Findings

- Act on `code`, never on `message`: codes are stable API (renaming one is a breaking
  change), messages follow upstream and may be reworded.
- `Code` is `#[non_exhaustive]`: a `match` needs a `_` arm.
- Prefixes: `HPT_CSV_*`, `HPT_JSON_*`, and `HPT_X_*` for Extended-only findings.
- `path` is a CSV cell (`"D5"`), a CSV row (`"row 5"`) or a JSON pointer.
- Optional fields carry the values, so nothing needs to be parsed out of a message:
  `field` (JSON), `column_name` and `value` (CSV), `allowed_values`, `versions`, `column`,
  `keyword` (the failed JSON Schema keyword for `HPT_JSON_SCHEMA`), `line` (Extended).
- `Finding::to_json()` serialises a finding the way upstream does, then `code`, `keyword`
  and `line`.

## Reading a result

- Row numbers in `path` count CSV rows; they differ from lines when a quoted field spans
  lines. Use Extended mode's `line` to point at the file.
- Some errors hide others: a blank row 3, fewer than four rows or a JSON syntax error
  replace every error found before them. Extended mode keeps those in `extended`.
- With a limit the lists stop at `max_errors`, and `valid()` still says whether any
  error was found.

The full code list with upstream classes is in the crate README (section "Codes").
