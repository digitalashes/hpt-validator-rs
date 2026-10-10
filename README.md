# hpt-validator

[![crates.io](https://img.shields.io/crates/v/hpt-validator.svg)](https://crates.io/crates/hpt-validator)
[![docs.rs](https://docs.rs/hpt-validator/badge.svg)](https://docs.rs/hpt-validator)
[![CI](https://github.com/digitalashes/hpt-validator-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/digitalashes/hpt-validator-rs/actions/workflows/ci.yml)

A Rust port of [`@cmsgov/hpt-validator`](https://github.com/CMSgov/hpt-validator), the
validator for CMS Hospital Price Transparency machine-readable files. It checks CSV files
(tall and wide) and JSON files against data dictionary versions 2.0.0, 2.1.0, 2.2.0 and
3.0.0, and returns the same errors and alerts as upstream.

> **Unofficial.** This project is not affiliated with, endorsed by, or maintained by the
> Centers for Medicare & Medicaid Services. It reproduces the behaviour of the upstream
> validator and does not speak for CMS.

- **Same result as upstream.** By default the report is what `@cmsgov/hpt-validator` 2.6.0
  returns through its CLI: the same findings in the same order with the same messages,
  upstream's quirks included. The test suite checks this against 2,184 results produced
  by upstream itself.
- **Streaming.** Rows and charge items are checked as they are read, so memory does not
  grow with the file.
- **No runtime dependencies.**
- **Made for programs.** Every finding has a stable code and its values as fields, so
  nothing has to be parsed out of a message. Extended mode adds what upstream drops or
  never reports.

## Usage

To validate files from a shell, use the command-line tool,
[`hpt-validator-cli`](https://github.com/digitalashes/hpt-validator-cli-rs). As a library:

```sh
cargo add hpt-validator
```

```rust,no_run
use hpt_validator::{Format, Mode, Options, validate};
use std::fs::File;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let file = File::open("123456789_hospital-name_standardcharges.csv")?;
    let options = Options { max_errors: 1000, mode: Mode::Default };
    let report = validate(file, Format::Csv, "3.0.0", &options)?;

    println!("valid: {}", report.valid());
    for error in &report.errors {
        println!("{}", error.to_json());
    }
    Ok(())
}
```

- The input is any `std::io::Read`: a file, stdin, a decompressor. Bytes are decoded as
  UTF-8 the way Node does it, with invalid sequences replaced.
- The version is read the way upstream reads it, so `"v3"` and `"3.0"` mean 3.0.0. An
  unknown version is a `HPT_CSV_INVALID_VERSION` finding for CSV and
  `Error::UnknownJsonVersion` for JSON, as upstream returns the first and throws the
  second.
- `max_errors` is upstream's `maxErrors`: validation stops once that many errors are
  found, and alerts are capped at the same number. `0` means no limit; upstream's CLI
  uses 1000.
- `validate_filename` checks a file name against the CMS naming convention,
  `<EIN>[-<NPI>]_<hospital name>_standardcharges.csv` or `.json`, like upstream's
  `validateFilename`.

## Findings

Errors and alerts are `Finding`s:

| Field | Meaning |
| --- | --- |
| `code` | What was found, as a stable code (below) |
| `path` | A CSV cell (`"D5"`), a CSV row (`"row 5"`) or a JSON pointer (`"/standard_charge_information/3"`) |
| `message` | Upstream's message |
| `field` | JSON: the last segment of the path within the item, as upstream sets it |
| `column_name`, `value`, `allowed_values`, `versions`, `column` | Upstream's properties for that kind of finding |
| `keyword` | JSON schema errors: the JSON Schema keyword that failed |
| `line` | Extended mode, CSV: the physical line where the finding's row starts |

`Finding::to_json` writes a finding the way upstream serialises it, keys in upstream's
order, followed by `code`, `keyword` and `line`:

```json
{"path":"D5","message":"A value is required for \"setting\". You must encode the missing information.","columnName":"setting","code":"HPT_CSV_REQUIRED_VALUE","line":5}
```

A report is valid when it has no errors; alerts never make it invalid.

## Extended mode

`Mode::Default` returns exactly what upstream returns. `Mode::Extended` returns the same
errors and alerts, unchanged, and adds:

- **`file_type`**: `Json`, `Tall`, `Wide`, or `Unknown` for a CSV file whose shape could
  not be determined.
- **`line`** on every CSV finding. Upstream counts rows, which drift from lines when a
  quoted field spans lines or when blank lines went with a byte order mark.
- **`extended`** findings:
  - errors upstream discards: a blank row 3, fewer than four rows and a JSON syntax error
    each replace every error found before them, and Extended keeps those errors here with
    their own codes;
  - malformed quotes, which upstream ignores although they merge fields:
    `HPT_X_CSV_INVALID_QUOTES` and `HPT_X_CSV_MISSING_QUOTES`;
  - blank lines removed together with a byte order mark, which shift every row number:
    `HPT_X_CSV_BOM_SWALLOWED_LINES`.

Extended findings are capped at `max_errors` separately. Removing `file_type`, `extended`
and `line` from an Extended report gives the Default report; the test suite checks this
on every case.

## Codes

Renaming a code is a breaking change; rewording a message is not.

| Code | Upstream class |
| --- | --- |
| `HPT_CSV_ALLOWED_VALUES` | `AllowedValuesError` |
| `HPT_CSV_AMBIGUOUS_FORMAT` | `AmbiguousFormatError` |
| `HPT_CSV_CHARGE_WITH_PAYER_PLAN` | `ChargeWithPayerPlanError` |
| `HPT_CSV_CODE_PAIR_MISSING` | `CodePairMissingError` |
| `HPT_CSV_COLUMN_MISSING` | `ColumnMissingError` |
| `HPT_CSV_DOLLAR_NEEDS_MIN_MAX` | `DollarNeedsMinMaxError` |
| `HPT_CSV_DRUG_INFORMATION_REQUIRED` | `DrugInformationRequiredError` |
| `HPT_CSV_DUPLICATE_COLUMN` | `DuplicateColumnError` |
| `HPT_CSV_DUPLICATE_HEADER_COLUMN` | `DuplicateHeaderColumnError` |
| `HPT_CSV_HEADER_BLANK` | `HeaderBlankError` |
| `HPT_CSV_HEADER_COLUMN_MISSING` | `HeaderColumnMissingError` |
| `HPT_CSV_INVALID_COUNT_NUMBER` | `InvalidCountNumberError` |
| `HPT_CSV_INVALID_DATE` | `InvalidDateError` |
| `HPT_CSV_INVALID_POSITIVE_NUMBER` | `InvalidPositiveNumberError` |
| `HPT_CSV_INVALID_STATE_CODE` | `InvalidStateCodeError` |
| `HPT_CSV_INVALID_VERSION` | `InvalidVersionError` |
| `HPT_CSV_ITEM_REQUIRES_CHARGE` | `ItemRequiresChargeError` |
| `HPT_CSV_MIN_ROWS` | `MinRowsError` |
| `HPT_CSV_MODIFIER_MISSING_INFO` | `ModifierMissingInfoError` |
| `HPT_CSV_OTHER_METHODOLOGY_NOTES` | `OtherMethodologyNotesError` |
| `HPT_CSV_ALLOWED_COUNT_ZERO_NOTES` | `AllowedCountZeroNotesError` |
| `HPT_CSV_PERCENTAGE_ALGORITHM_ESTIMATE` | `PercentageAlgorithmEstimateError` |
| `HPT_CSV_PERCENTAGE_ALGORITHM_COUNT` | `PercentageAlgorithmCountError` |
| `HPT_CSV_PERCENTAGE_ALGORITHM_MEDIAN` | `PercentageAlgorithmMedianError` |
| `HPT_CSV_PERCENTAGE_ALGORITHM_10TH` | `PercentageAlgorithm10thError` |
| `HPT_CSV_PERCENTAGE_ALGORITHM_90TH` | `PercentageAlgorithm90thError` |
| `HPT_CSV_PLACEHOLDER` | `PlaceholderError` |
| `HPT_CSV_PROBLEMS_IN_HEADER` | `ProblemsInHeaderError` |
| `HPT_CSV_REQUIRED_VALUE` | `RequiredValueError` |
| `HPT_CSV_ESTIMATED_AMOUNT_DEPRECATED` | `EstimatedAmountAlert` |
| `HPT_CSV_FALSE_AFFIRMATION` | `CsvFalseAffirmationAlert` |
| `HPT_CSV_FALSE_ATTESTATION` | `CsvFalseAttestationAlert` |
| `HPT_CSV_NINE_NINES` | `CsvNineNinesAlert` |
| `HPT_CSV_VERSION_MISMATCH` | `CsvVersionMismatchAlert` |
| `HPT_JSON_INVALID` | `InvalidJsonError` |
| `HPT_JSON_SCHEMA` | Any schema error; `keyword` says which rule failed |
| `HPT_JSON_ESTIMATED_AMOUNT_DEPRECATED` | Alert schema: `estimated_amount` present |
| `HPT_JSON_NINE_NINES` | Alert schema: an amount of 999999999 |
| `HPT_JSON_FALSE_AFFIRMATION` | `JsonFalseAffirmationAlert` |
| `HPT_JSON_FALSE_ATTESTATION` | `JsonFalseAttestationAlert` |
| `HPT_JSON_VERSION_MISMATCH` | `JsonVersionMismatchAlert` |
| `HPT_X_CSV_INVALID_QUOTES` | Extended: a quote that neither closes a field nor is doubled |
| `HPT_X_CSV_MISSING_QUOTES` | Extended: a quoted field that is never closed |
| `HPT_X_CSV_BOM_SWALLOWED_LINES` | Extended: blank lines removed with the byte order mark |

## Compatibility

The port is checked against `@cmsgov/hpt-validator` 2.6.0 at commit
[`5b7c249`](https://github.com/CMSgov/hpt-validator/commit/5b7c249df226c28da62a0d9fa0b0308207ae1aac),
pinned in `UPSTREAM.lock`, run on Node 20.19.4. For every input in the test suite,
`Mode::Default` returns the same errors and alerts as upstream, in the same order, with the
same messages and properties. Upstream's quirks that show in a result are kept, and
Extended mode adds what some of them hide; those that cannot change a result, such as
repeated work, are not kept.

Known differences:

- Lower-casing a final Greek sigma follows Rust's Unicode 17 character classes, which
  differ from Node 20's at about 46 code points. It matters only for wide-format payer and
  plan names that contain such text.
- Upstream reports two missing columns for every code number up to the highest one in a
  CSV's row 3, so `code | 99999999` asks for 200 million errors. It fails with no result
  past 2^32 - 1 codes, and when more than about 121,000 of those errors get through
  `max_errors` at once, which overflows its call stack. The port returns a result there;
  with `max_errors` at `0`, it lists the missing columns of at most 65,536 code numbers
  past the row's columns. Whenever upstream returns a result, the port's is the same.
- Upstream's `onValueCallback` and `onMetadataCallback` are not provided. They do not
  affect the result.

## How parity is tested

`oracle/` runs upstream, pinned in `UPSTREAM.lock`, on Node 20.19.4 over every input in
`tests/cases`: upstream's own fixtures, the scenarios of upstream's unit tests turned into
files, and inputs for each upstream quirk the port reproduces. It writes upstream's
results to `tests/golden`, along with JavaScript semantics tables (`tests/js.json`) and
the CSV parser's rows (`tests/reader`). `cargo test` compares the port with all of them in
both modes. Upstream cannot produce the Extended additions, so `tests/extended.json` pins
them for a set of those inputs, reviewed by hand.

`tests/fuzz.rs` mutates every input at random and checks that the validator does not panic
and that Extended only adds to Default; `fuzz/` runs the same check under
[cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) on a nightly toolchain:
`mkdir -p fuzz/corpus/csv`, then
`cargo +nightly fuzz run -s none csv fuzz/corpus/csv tests/cases` (or `json`).
`tests/memory.rs` validates a generated 32 MiB file of each format and checks that
resident memory stays flat.

To regenerate the expected results, put Node 20.19.4 on `PATH` and run `oracle/run.sh`;
it clones and builds upstream under `target/upstream`. A weekly CI job fails once
upstream's default branch moves past the commit in `UPSTREAM.lock`.

The suite is run on the latest stable Rust. Case mapping has to match Node 20's Unicode
tables, so a test fails when a Rust release moves to a new Unicode version.

## License

[CC0 1.0](https://github.com/digitalashes/hpt-validator-rs/blob/main/LICENSE), like upstream. `schemas/` holds the CMS JSON Schemas, copied
unchanged from upstream at the commit in `UPSTREAM.lock`.
