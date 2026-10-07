//! Data rows: upstream's row validators and alerters in the order upstream builds them
//! (design §2.2), reading the columns row 3 resolved.

use super::{allowed_values, amounts, at, required_value, same};
use crate::{Code, Finding, Version, js};

/// Upstream's `BILLING_CODE_TYPES`, then the two 3.0 adds.
const BILLING_CODES: &[&str] = &[
    "CPT",
    "HCPCS",
    "ICD",
    "DRG",
    "MS-DRG",
    "R-DRG",
    "S-DRG",
    "APS-DRG",
    "AP-DRG",
    "APR-DRG",
    "APC",
    "NDC",
    "HIPPS",
    "LOCAL",
    "EAPG",
    "CDT",
    "RC",
    "CDM",
    "TRIS-DRG",
    "CMG",
    "MS-LTC-DRG",
];

const SETTINGS: &[&str] = &["inpatient", "outpatient", "both"];

const METHODOLOGIES: &[&str] = &[
    "case rate",
    "fee schedule",
    "percent of total billed charges",
    "per diem",
    "other",
];

const DRUG_UNITS: &[&str] = &["GR", "ME", "ML", "UN", "F2", "EA", "GM"];

const PAYER_SPECIFIC: &str = " when a payer specific negotiated charge is encoded as a dollar amount, percentage, or algorithm";

/// Row 3's columns as the rules read them, by index. Once row 3 passes, every column a rule
/// reads is present.
pub struct Layout {
    pub v: Version,
    /// Row 3 as written, which messages quote.
    pub names: Vec<String>,
    pub description: usize,
    pub setting: usize,
    /// `code | i` and `code | i | type`.
    pub codes: Vec<(usize, usize)>,
    /// `standard_charge | gross`, `discounted_cash`, `min`, `max`.
    pub standard: [usize; 4],
    /// `additional_generic_notes`.
    pub notes: usize,
    /// Tall only: `payer_name`, `plan_name`.
    pub payer_plan: Option<(usize, usize)>,
    pub groups: Vec<Group>,
    /// 2.2 and later: `drug_unit_of_measurement`, `drug_type_of_measurement`, `modifiers`.
    pub drug: Option<[usize; 3]>,
}

/// One payer/plan's charge columns: tall's only group, or one per wide payer/plan.
pub struct Group {
    /// `negotiated_dollar`, `negotiated_percentage`, `negotiated_algorithm`, `methodology`.
    pub charges: [usize; 4],
    /// `additional_generic_notes` (tall) or `additional_payer_notes | payer | plan` (wide).
    pub notes: usize,
    /// The columns of `amounts(v)`: `estimated_amount` (2.2), or median, 10th, 90th and
    /// count (3.0).
    pub amounts: Vec<usize>,
}

/// One data row being checked.
struct Check<'a> {
    l: &'a Layout,
    row: u64,
    values: &'a [&'a str],
    found: Vec<Finding>,
}

/// Upstream's `rowValidators` on one data row.
pub fn errors(l: &Layout, row: u64, values: &[&str]) -> Vec<Finding> {
    let mut c = Check::new(l, row, values);
    let v = l.v;
    c.required(l.description, "");
    let billing_codes = if v == Version::V3_0 {
        BILLING_CODES
    } else {
        &BILLING_CODES[..BILLING_CODES.len() - 2]
    };
    for &(code, ty) in &l.codes {
        if c.has(ty) {
            c.required(code, "");
        }
        if c.has(code) {
            c.required(ty, "");
            c.one_of(ty, billing_codes);
        }
    }
    for i in l.standard {
        c.positive(i);
    }
    for g in &l.groups {
        let [dollar, percentage, _, methodology] = g.charges;
        c.positive(dollar);
        c.positive(percentage);
        c.one_of(methodology, METHODOLOGIES);
        for (&amount, &i) in amounts(v).iter().zip(&g.amounts) {
            if amount == "count" {
                c.count(i);
            } else {
                c.positive(i);
            }
        }
    }
    if let Some([unit, ty, _]) = l.drug {
        if c.has(unit) || c.has(ty) {
            c.required(unit, " when \"drug_type_of_measurement\" is present");
            c.positive(unit);
            c.required(ty, " when \"drug_unit_of_measurement\" is present");
            c.one_of(ty, DRUG_UNITS);
        }
        if l.codes.iter().any(|&(_, ty)| same(c.value(ty), "NDC"))
            && let Some(i) = [unit, ty].into_iter().find(|&i| !c.has(i))
        {
            let message = "If code type is NDC, then the corresponding drug unit of measure and drug type of measure data element must be encoded.";
            c.error(Code::CsvDrugInformationRequired, Some(i), message);
        }
    }
    // Before 2.2 a row without codes is an error; from 2.2 it may be a modifier row.
    let coded = l.codes.iter().any(|&(code, ty)| c.has(code) || c.has(ty));
    match l.drug {
        Some([.., modifiers]) if !coded && c.has(modifiers) => c.modifier(),
        _ => {
            if !coded {
                let message = "If a standard charge is encoded, there must be a corresponding code and code type pairing. The code and code type pairing do not need to be in the first code and code type columns (i.e., code|1 and code|1|type).";
                c.error(Code::CsvCodePairMissing, None, message);
            }
            c.non_modifier();
        }
    }
    c.found
}

/// Upstream's `rowAlerters` on one data row: nine 9s in an allowed amount.
pub fn alerts(l: &Layout, row: u64, values: &[&str]) -> Vec<Finding> {
    let mut c = Check::new(l, row, values);
    for g in &l.groups {
        let amounts: &[_] = match g.amounts[..] {
            [estimated] => &[(estimated, "estimated amount")],
            [median, p10, p90, count] => &[
                (count, "count"),
                (median, "median amount"),
                (p10, "10th percentile"),
                (p90, "90th percentile"),
            ],
            _ => &[],
        };
        for &(i, name) in amounts {
            if js::number(c.value(i)) == 999999999.0 {
                c.error(
                    Code::CsvNineNines,
                    Some(i),
                    &format!("Nine 9s used for {name}."),
                );
            }
        }
    }
    c.found
}

impl<'a> Check<'a> {
    fn new(l: &'a Layout, row: u64, values: &'a [&'a str]) -> Self {
        Check {
            l,
            row,
            values,
            found: Vec::new(),
        }
    }

    /// Missing cells read as empty.
    fn value(&self, i: usize) -> &'a str {
        self.values.get(i).copied().unwrap_or("")
    }

    fn has(&self, i: usize) -> bool {
        !self.value(i).is_empty()
    }

    fn name(&self, i: usize) -> &'a str {
        &self.l.names[i]
    }

    fn error(&mut self, code: Code, column: Option<usize>, message: &str) {
        let finding = at(code, self.row, column, message.into());
        self.found.push(finding);
    }

    /// A `RequiredValueError` for a missing value. The other checks pass a missing value,
    /// so upstream's required variants are this plus the optional one.
    fn required(&mut self, i: usize, suffix: &str) {
        if !self.has(i) {
            let finding = required_value(self.row, i, self.name(i), suffix);
            self.found.push(finding);
        }
    }

    /// An `AllowedValuesError` for a present value outside `allowed`.
    fn one_of(&mut self, i: usize, allowed: &'static [&'static str]) {
        let value = self.value(i);
        if !value.is_empty() && !allowed.iter().any(|a| same(value, a)) {
            let finding = allowed_values(self.row, i, self.name(i), value, allowed);
            self.found.push(finding);
        }
    }

    /// An `InvalidPositiveNumberError` for a present value that is not a positive number.
    fn positive(&mut self, i: usize) {
        let value = self.value(i);
        if value.is_empty() || js::is_positive_number(value) {
            return;
        }
        let message = format!(
            "\"{}\" value \"{value}\" is not a positive number. You must encode a positive, non-zero, numeric value.",
            self.name(i)
        );
        self.found.push(Finding {
            column_name: Some(self.name(i).into()),
            value: Some(value.into()),
            ..at(Code::CsvInvalidPositiveNumber, self.row, Some(i), message)
        });
    }

    /// A count of allowed amounts: `0`, `1 through 10`, or
    /// `/^(1[1-9]|[2-9]\d+|[1-9]\d{2,})$/`, an integer from 11 without leading zeros.
    fn count(&mut self, i: usize) {
        let value = self.value(i);
        let integer = value.len() >= 2
            && value.bytes().all(|b| b.is_ascii_digit())
            && !value.starts_with('0')
            && value != "10";
        if value.is_empty() || same(value, "0") || same(value, "1 through 10") || integer {
            return;
        }
        let message = format!(
            "\"{}\" value \"{value}\" is not a valid count of allowed amounts. You must encode 0, an integer 11 or greater, or \"1 through 10\".",
            self.name(i)
        );
        self.found.push(Finding {
            column_name: Some(self.name(i).into()),
            value: Some(value.into()),
            ..at(Code::CsvInvalidCountNumber, self.row, Some(i), message)
        });
    }

    /// Whether the group has a payer-specific charge: dollar, percentage or algorithm.
    fn charged(&self, g: &Group) -> bool {
        g.charges[..3].iter().any(|&i| self.has(i))
    }

    /// Upstream's `nonModifierChecks`.
    fn non_modifier(&mut self) {
        let l = self.l;
        self.required(l.setting, "");
        self.one_of(l.setting, SETTINGS);
        if l.v < Version::V2_1 {
            return;
        }
        for g in &l.groups {
            let methodology = g.charges[3];
            if self.charged(g) {
                if let Some((payer, plan)) = l.payer_plan {
                    self.required(payer, PAYER_SPECIFIC);
                    self.required(plan, PAYER_SPECIFIC);
                }
                self.required(methodology, PAYER_SPECIFIC);
            }
            if same(self.value(methodology), "other") && !self.has(g.notes) {
                let message = notes_required("standard charge methodology", "other");
                self.error(Code::CsvOtherMethodologyNotes, Some(g.notes), &message);
            }
        }
        let [gross, discounted, min, max] = l.standard;
        if !self.has(gross) && !self.has(discounted) && !l.groups.iter().any(|g| self.charged(g)) {
            let message = "If an item or service is encoded, a corresponding valid value must be encoded for at least one of the following: \"Gross Charge\", \"Discounted Cash Price\", \"Payer-Specific Negotiated Charge: Dollar Amount\", \"Payer-Specific Negotiated Charge: Percentage\", \"Payer-Specific Negotiated Charge: Algorithm\".";
            self.error(Code::CsvItemRequiresCharge, Some(gross), message);
        }
        if l.groups.iter().any(|g| self.has(g.charges[0]))
            && let Some(i) = [min, max].into_iter().find(|&i| !self.has(i))
        {
            let message = "If there is a \"payer specific negotiated charge\" encoded as a dollar amount, there must be a corresponding valid value encoded for the deidentified minimum and deidentified maximum negotiated charge data.";
            self.error(Code::CsvDollarNeedsMinMax, Some(i), message);
        }
        for g in &l.groups {
            let [dollar, percentage, algorithm, _] = g.charges;
            let ratio = self.has(percentage) || self.has(algorithm);
            match g.amounts[..] {
                [estimated] => {
                    if !self.has(dollar) && ratio && !self.has(estimated) {
                        let message = "If a \"payer specific negotiated charge\" can only be expressed as a percentage or algorithm, then a corresponding \"Estimated Allowed Amount\" must also be encoded.";
                        self.error(
                            Code::CsvPercentageAlgorithmEstimate,
                            Some(estimated),
                            message,
                        );
                    }
                }
                [median, p10, p90, count] => {
                    if ratio && !self.has(count) {
                        let message = "If a \"payer specific negotiated charge\" is expressed as a percentage or algorithm, then a corresponding \"Count of Allowed Amounts\" must also be encoded.";
                        self.error(Code::CsvPercentageAlgorithmCount, Some(count), message);
                    }
                    let zero = self.value(count) == "0";
                    if ratio && !zero {
                        for (i, code, field) in [
                            (median, Code::CsvPercentageAlgorithmMedian, "Median Amount"),
                            (p10, Code::CsvPercentageAlgorithm10th, "10th Percentile"),
                            (p90, Code::CsvPercentageAlgorithm90th, "90th Percentile"),
                        ] {
                            if !self.has(i) {
                                let message = format!(
                                    "If a \"payer specific negotiated charge\" is expressed as a percentage or algorithm, and count of allowed amounts is not zero, then a corresponding \"{field}\" must also be encoded."
                                );
                                self.error(code, Some(i), &message);
                            }
                        }
                    }
                    if zero && !self.has(g.notes) {
                        let message = notes_required("count of allowed amounts", "0");
                        self.error(Code::CsvAllowedCountZeroNotes, Some(g.notes), &message);
                    }
                }
                _ => {}
            }
        }
        if l.v == Version::V3_0
            && let Some((payer, plan)) = l.payer_plan
            && self.has(payer)
            && self.has(plan)
            && !self.charged(&l.groups[0])
        {
            let message = "If a payer name and plan name are encoded, a payer-specific charge must also be encoded.";
            self.error(Code::CsvChargeWithPayerPlan, Some(payer), message);
        }
    }

    /// Upstream's `modifierChecks`: a modifier row needs notes or a payer-specific charge.
    fn modifier(&mut self) {
        let l = self.l;
        let info =
            self.has(l.notes) || (l.groups.iter()).any(|g| self.charged(g) || self.has(g.notes));
        if !info {
            let message = "If a modifier is encoded without an item or service, then a description and one of the following is the minimum information required: additional_payer_notes, standard_charge | negotiated_dollar, standard_charge | negotiated_percentage, or standard_charge | negotiated_algorithm.";
            self.error(Code::CsvModifierMissingInfo, Some(l.notes), message);
        }
    }
}

fn notes_required(element: &str, value: &str) -> String {
    format!(
        "If the \"{element}\" encoded value is \"{value}\", there must be a corresponding explanation found in the \"additional notes\" for the associated payer-specific negotiated charge."
    )
}
