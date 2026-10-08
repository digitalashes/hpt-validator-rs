//! Data rows: upstream's row validators and alerters in the order upstream builds them
//! (design §2.2), reading the columns row 3 resolved.

use super::{allowed_values, at, invalid_value, required_value, same};
use crate::{Code, Finding, Version, js};

/// Upstream's `BILLING_CODE_TYPES`, then the two 3.0 adds.
const BILLING_CODES: &[&str] = &[
    "CPT", "HCPCS", "ICD", "DRG", "MS-DRG", "R-DRG", "S-DRG", "APS-DRG", "AP-DRG", "APR-DRG",
    "APC", "NDC", "HIPPS", "LOCAL", "EAPG", "CDT", "RC", "CDM", "TRIS-DRG", "CMG", "MS-LTC-DRG",
];

const SETTINGS: &[&str] = &["inpatient", "outpatient", "both"];

const METHODOLOGIES: &[&str] =
    &["case rate", "fee schedule", "percent of total billed charges", "per diem", "other"];

const DRUG_UNITS: &[&str] = &["GR", "ME", "ML", "UN", "F2", "EA", "GM"];

const PAYER_SPECIFIC: &str = " when a payer specific negotiated charge is encoded as a dollar amount, percentage, or algorithm";

/// Row 3's columns as the rules read them. Every other `usize` here is a slot in upstream's
/// expected column list, which `columns` maps to its row-3 column; once row 3 passes, each
/// slot has one.
pub struct Layout {
    pub v: Version,
    pub columns: Vec<usize>,
    /// Each slot's column as written in row 3, which messages quote.
    pub names: Vec<String>,
    pub description: usize,
    pub setting: usize,
    /// `code | i` and `code | i | type`.
    pub codes: Vec<(usize, usize)>,
    /// `standard_charge | gross`, `discounted_cash`, `min`, `max`.
    pub standard: [usize; 4],
    /// `additional_generic_notes`.
    pub notes: usize,
    pub groups: Vec<Group>,
    /// 2.2 and later: `drug_unit_of_measurement`, `drug_type_of_measurement`.
    pub drug: Option<[usize; 2]>,
    /// 2.2 and later.
    pub modifiers: Option<usize>,
}

/// One payer/plan's charge columns: tall's only group, or one per wide payer/plan.
pub struct Group {
    pub dollar: usize,
    pub percentage: usize,
    pub algorithm: usize,
    pub methodology: usize,
    /// `additional_generic_notes` (tall) or `additional_payer_notes | payer | plan` (wide).
    pub notes: usize,
    /// Tall only: `payer_name`, `plan_name`.
    pub payer_plan: Option<[usize; 2]>,
    pub amounts: Amounts,
    /// The row-3 columns of all the slots above: a row with each of them empty passes every
    /// rule on the group.
    pub columns: Vec<usize>,
}

impl Group {
    /// Every slot above.
    pub fn slots(&self) -> impl Iterator<Item = usize> + '_ {
        let amounts: &[usize] = match &self.amounts {
            Amounts::None => &[],
            Amounts::Estimated(estimated) => std::slice::from_ref(estimated),
            Amounts::Allowed(allowed) => allowed,
        };
        [self.dollar, self.percentage, self.algorithm, self.methodology, self.notes]
            .into_iter()
            .chain(self.payer_plan.into_iter().flatten())
            .chain(amounts.iter().copied())
    }
}

/// A group's allowed-amount columns, which the version decides.
#[derive(Clone, Copy)]
pub enum Amounts {
    None,
    /// 2.2: `estimated_amount`.
    Estimated(usize),
    /// 3.0: `median_amount`, `10th_percentile`, `90th_percentile`, `count`.
    Allowed([usize; 4]),
}

/// One data row being checked.
struct Check<'a> {
    l: &'a Layout,
    /// The row's groups with a value (see `live`).
    live: &'a [usize],
    row: u64,
    values: &'a [&'a str],
    found: Vec<Finding>,
}

/// The groups with a value in a row, by index, for `errors` and `alerts` on that row: the
/// rules pass a group without one, and a wide row leaves most of its groups empty.
pub fn live(l: &Layout, values: &[&str], groups: &mut Vec<usize>) {
    let has = |&i: &usize| values.get(i).is_some_and(|v| !v.is_empty());
    groups.clear();
    groups.extend(
        (l.groups.iter().enumerate()).filter(|(_, g)| g.columns.iter().any(has)).map(|(i, _)| i),
    );
}

/// Upstream's `rowValidators` on one data row.
pub fn errors(l: &Layout, live: &[usize], row: u64, values: &[&str]) -> Vec<Finding> {
    let mut c = Check::new(l, live, row, values);
    c.required(l.description, "");
    let billing_codes = if l.v == Version::V3_0 {
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
    for g in c.groups() {
        c.positive(g.dollar);
        c.positive(g.percentage);
        c.one_of(g.methodology, METHODOLOGIES);
        match g.amounts {
            Amounts::None => {}
            Amounts::Estimated(estimated) => c.positive(estimated),
            Amounts::Allowed([median, p10, p90, count]) => {
                c.positive(median);
                c.positive(p10);
                c.positive(p90);
                c.count(count);
            }
        }
    }
    if let Some([unit, ty]) = l.drug {
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
    if !coded && l.modifiers.is_some_and(|m| c.has(m)) {
        c.modifier();
    } else {
        if !coded {
            let message = "If a standard charge is encoded, there must be a corresponding code and code type pairing. The code and code type pairing do not need to be in the first code and code type columns (i.e., code|1 and code|1|type).";
            c.error(Code::CsvCodePairMissing, None, message);
        }
        c.non_modifier();
    }
    c.found
}

/// Upstream's `rowAlerters` on one data row: nine 9s in an allowed amount.
pub fn alerts(l: &Layout, live: &[usize], row: u64, values: &[&str]) -> Vec<Finding> {
    let mut c = Check::new(l, live, row, values);
    for g in c.groups() {
        let amounts: &[_] = match g.amounts {
            Amounts::None => &[],
            Amounts::Estimated(estimated) => &[(estimated, "estimated amount")],
            Amounts::Allowed([median, p10, p90, count]) => &[
                (count, "count"),
                (median, "median amount"),
                (p10, "10th percentile"),
                (p90, "90th percentile"),
            ],
        };
        for &(i, name) in amounts {
            #[expect(clippy::float_cmp, reason = "JavaScript's ===")]
            let nines = js::number(c.value(i)) == 999_999_999.0;
            if nines {
                c.error(Code::CsvNineNines, Some(i), &format!("Nine 9s used for {name}."));
            }
        }
    }
    c.found
}

impl<'a> Check<'a> {
    fn new(l: &'a Layout, live: &'a [usize], row: u64, values: &'a [&'a str]) -> Self {
        Check { l, live, row, values, found: Vec::new() }
    }

    fn groups(&self) -> impl Iterator<Item = &'a Group> + use<'a> {
        let (groups, live) = (&self.l.groups, self.live);
        live.iter().map(move |&i| &groups[i])
    }

    /// The slot's cell; missing cells read as empty.
    fn value(&self, slot: usize) -> &'a str {
        let i = self.l.columns[slot];
        self.values.get(i).copied().unwrap_or("")
    }

    fn has(&self, slot: usize) -> bool {
        !self.value(slot).is_empty()
    }

    /// The slot's row-3 column and its name there.
    fn column(&self, slot: usize) -> (usize, &'a str) {
        (self.l.columns[slot], &self.l.names[slot])
    }

    fn error(&mut self, code: Code, slot: Option<usize>, message: &str) {
        let column = slot.map(|slot| self.l.columns[slot]);
        let finding = at(code, self.row, column, message.into());
        self.found.push(finding);
    }

    /// A `RequiredValueError` for a missing value. The other checks pass a missing value,
    /// so upstream's required variants are this plus the optional one.
    fn required(&mut self, slot: usize, suffix: &str) {
        if !self.has(slot) {
            let (i, name) = self.column(slot);
            self.found.push(required_value(self.row, i, name, suffix));
        }
    }

    /// An `AllowedValuesError` for a present value outside `allowed`.
    fn one_of(&mut self, slot: usize, allowed: &'static [&'static str]) {
        let value = self.value(slot);
        // Allowed values are ASCII, so an ASCII value is compared without case mapping.
        let ascii = value.is_ascii();
        let known = |a: &&str| if ascii { a.eq_ignore_ascii_case(value) } else { same(value, a) };
        if !value.is_empty() && !allowed.iter().any(known) {
            let (i, name) = self.column(slot);
            self.found.push(allowed_values(self.row, i, name, value, allowed));
        }
    }

    /// An `InvalidPositiveNumberError` for a present value that is not a positive number.
    fn positive(&mut self, slot: usize) {
        let value = self.value(slot);
        if !value.is_empty() && !js::is_positive_number(value) {
            let (i, name) = self.column(slot);
            let problem =
                "is not a positive number. You must encode a positive, non-zero, numeric value.";
            let code = Code::CsvInvalidPositiveNumber;
            self.found.push(invalid_value(code, self.row, i, name, value, problem));
        }
    }

    /// A count of allowed amounts: `0`, `1 through 10` in any case, or an integer from 11.
    fn count(&mut self, slot: usize) {
        let value = self.value(slot);
        if value.is_empty()
            || same(value, "0")
            || same(value, "1 through 10")
            || crate::is_count_integer(value)
        {
            return;
        }
        let (i, name) = self.column(slot);
        let problem = "is not a valid count of allowed amounts. You must encode 0, an integer 11 or greater, or \"1 through 10\".";
        let code = Code::CsvInvalidCountNumber;
        self.found.push(invalid_value(code, self.row, i, name, value, problem));
    }

    /// Whether the group has a payer-specific charge: dollar, percentage or algorithm.
    fn charged(&self, g: &Group) -> bool {
        [g.dollar, g.percentage, g.algorithm].into_iter().any(|i| self.has(i))
    }

    /// Upstream's `nonModifierChecks`.
    fn non_modifier(&mut self) {
        let l = self.l;
        self.required(l.setting, "");
        self.one_of(l.setting, SETTINGS);
        if l.v < Version::V2_1 {
            return;
        }
        for g in self.groups() {
            if self.charged(g) {
                if let Some([payer, plan]) = g.payer_plan {
                    self.required(payer, PAYER_SPECIFIC);
                    self.required(plan, PAYER_SPECIFIC);
                }
                self.required(g.methodology, PAYER_SPECIFIC);
            }
            if same(self.value(g.methodology), "other") && !self.has(g.notes) {
                let message = notes_required("standard charge methodology", "other");
                self.error(Code::CsvOtherMethodologyNotes, Some(g.notes), &message);
            }
        }
        let [gross, discounted, min, max] = l.standard;
        if !self.has(gross) && !self.has(discounted) && !self.groups().any(|g| self.charged(g)) {
            let message = "If an item or service is encoded, a corresponding valid value must be encoded for at least one of the following: \"Gross Charge\", \"Discounted Cash Price\", \"Payer-Specific Negotiated Charge: Dollar Amount\", \"Payer-Specific Negotiated Charge: Percentage\", \"Payer-Specific Negotiated Charge: Algorithm\".";
            self.error(Code::CsvItemRequiresCharge, Some(gross), message);
        }
        if self.groups().any(|g| self.has(g.dollar))
            && let Some(i) = [min, max].into_iter().find(|&i| !self.has(i))
        {
            let message = "If there is a \"payer specific negotiated charge\" encoded as a dollar amount, there must be a corresponding valid value encoded for the deidentified minimum and deidentified maximum negotiated charge data.";
            self.error(Code::CsvDollarNeedsMinMax, Some(i), message);
        }
        for g in self.groups() {
            let ratio = self.has(g.percentage) || self.has(g.algorithm);
            match g.amounts {
                Amounts::None => {}
                Amounts::Estimated(estimated) => {
                    if !self.has(g.dollar) && ratio && !self.has(estimated) {
                        let message = "If a \"payer specific negotiated charge\" can only be expressed as a percentage or algorithm, then a corresponding \"Estimated Allowed Amount\" must also be encoded.";
                        self.error(Code::CsvPercentageAlgorithmEstimate, Some(estimated), message);
                    }
                }
                Amounts::Allowed([median, p10, p90, count]) => {
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
            }
            if l.v == Version::V3_0
                && let Some([payer, plan]) = g.payer_plan
                && self.has(payer)
                && self.has(plan)
                && !self.charged(g)
            {
                let message = "If a payer name and plan name are encoded, a payer-specific charge must also be encoded.";
                self.error(Code::CsvChargeWithPayerPlan, Some(payer), message);
            }
        }
    }

    /// Upstream's `modifierChecks`: a modifier row needs notes or a payer-specific charge.
    fn modifier(&mut self) {
        let l = self.l;
        let info = self.has(l.notes) || self.groups().any(|g| self.charged(g) || self.has(g.notes));
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
