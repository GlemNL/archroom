//! The `Criterion` filter registry (plan §4.3: "Library filter or
//! smart-collection rule | `Criterion` | `catalog::criteria` | A SQL
//! fragment builder and a UI descriptor").
//!
//! Two M2 features share it:
//! - the Library filter bar (plan §7.5) turns its state into a `Vec<Rule>`,
//! - a smart collection stores a `SmartRules` JSON blob in
//!   `collections.rules` (plan §7.2).
//!
//! Every value-critical SQL lives here, in the trait impl of one tiny
//! stateless type per criterion; adding a criterion means one type plus
//! one line in [`registry`], nothing else. Fragments target the
//! `photos p JOIN files f` rows the grid queries use (plan §5.1), with
//! `?N` parameter numbering starting at whatever `base` the caller
//! supplies so a source clause can hold `?1` before the criteria run.

use rusqlite::types::Value;
use serde::{Deserialize, Serialize};

/// Comparison semantics for the scalar criteria (plan §7.5's "≥ / = / ≤").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RelOp {
    AtLeast,
    Exactly,
    AtMost,
}

impl RelOp {
    fn sign(self) -> &'static str {
        match self {
            RelOp::AtLeast => ">=",
            RelOp::Exactly => "=",
            RelOp::AtMost => "<=",
        }
    }
}

/// The flag attribute (plan §5.1: -1 reject, 0 none, 1 pick).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FlagValue {
    Rejected,
    Unflagged,
    Picked,
}

impl FlagValue {
    fn as_i64(self) -> i64 {
        match self {
            FlagValue::Rejected => -1,
            FlagValue::Unflagged => 0,
            FlagValue::Picked => 1,
        }
    }
}

/// One filterable property snapshot. Smart collections persist these
/// through serde; the filter bar builds them from `LibraryFilter`.
#[derive(Debug, Clone, PartialEq, Hash, Serialize, Deserialize)]
pub enum Rule {
    /// Filename, title, caption or keywords contain every whitespace
    /// token (FTS5 prefix match, plan §7.5's text filter).
    Text {
        contains: String,
    },
    Rating {
        op: RelOp,
        stars: u8,
    },
    Flag {
        value: FlagValue,
    },
    /// `None` = has no color label; `Some(name)` = has that label.
    Label {
        label: Option<String>,
    },
    /// Has a `develop_settings` row (plan §7.5's "edited or unedited").
    Edited {
        edited: bool,
    },
    VirtualCopy {
        is_copy: bool,
    },
    CaptureYear {
        year: u16,
    },
    CaptureMonth {
        year: u16,
        month: u8,
    },
    CaptureDay {
        year: u16,
        month: u8,
        day: u8,
    },
    Camera {
        model: String,
    },
    Lens {
        contains: String,
    },
}

impl Rule {
    /// `describe`/`where` for a rule are the matching criterion's job; an
    /// unknown rule (a future version's JSON in a catalog) degrades to
    /// matching nothing rather than crashing a load.
    pub fn describe(&self) -> String {
        let Some(c) = criterion_for(self) else {
            return "Unknown rule".to_string();
        };
        c.describe(self)
    }

    /// The WHERE fragment for this rule, with `?N` numbering starting at
    /// `base`. The empty string means "matches everything" (an unknown
    /// rule).
    pub fn sql(&self, base: i32) -> (String, Vec<Value>) {
        let Some(c) = criterion_for(self) else {
            return (String::new(), Vec::new());
        };
        c.sql(self, base)
    }
}

/// The registry key for a criterion ("rating", "text", …).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CriterionId(pub &'static str);

impl CriterionId {
    pub fn get(self) -> &'static str {
        self.0
    }
}

/// One filterable property (plan §4.4's `Criterion` sketch: a SQL fragment
/// builder and a UI descriptor — the UI descriptor is [`Criterion::label`]
/// plus the [`registry`] order, which the panels render).
pub trait Criterion: Send + Sync + 'static {
    fn id(&self) -> CriterionId;

    /// Shown in the filter bar and smart-collection rule editor.
    fn label(&self) -> &str;

    /// Does this criterion own `rule`? The registry lookup uses this so
    /// adding a criterion needs no match statement anywhere.
    fn applies(&self, _rule: &Rule) -> bool {
        false
    }

    /// A human phrase, e.g. `"Rating ≥ 3"` — what a smart-collection row
    /// and a filter-bar chip display.
    fn describe(&self, _rule: &Rule) -> String {
        String::new()
    }

    /// A WHERE fragment against `photos p JOIN files f` and its `?N`
    /// parameters, numbered from `base`.
    fn sql(&self, _rule: &Rule, _base: i32) -> (String, Vec<Value>) {
        (String::new(), Vec::new())
    }
}

/// Every criterion, in panel display order. A new criterion is one unit
/// type plus one line here.
pub fn registry() -> Vec<Box<dyn Criterion>> {
    vec![
        Box::new(TextCriterion),
        Box::new(RatingCriterion),
        Box::new(FlagCriterion),
        Box::new(LabelCriterion),
        Box::new(EditedCriterion),
        Box::new(VirtualCopyCriterion),
        Box::new(CaptureDateCriterion),
        Box::new(CameraCriterion),
        Box::new(LensCriterion),
    ]
}

/// Looks up the criterion owning `rule`, or `None` for an unknown rule.
pub fn criterion_for(rule: &Rule) -> Option<Box<dyn Criterion>> {
    registry().into_iter().find(|c| c.applies(rule))
}

/// The criterion id that owns `rule`, if any.
pub fn rule_criterion_id(rule: &Rule) -> Option<CriterionId> {
    criterion_for(rule).map(|c| c.id())
}

// ---------------------------------------------------------------------
// The individual criteria
// ---------------------------------------------------------------------

#[derive(Debug)]
pub struct TextCriterion;
impl Criterion for TextCriterion {
    fn id(&self) -> CriterionId {
        CriterionId("text")
    }

    fn label(&self) -> &str {
        "Text"
    }

    fn applies(&self, rule: &Rule) -> bool {
        matches!(rule, Rule::Text { .. })
    }

    fn describe(&self, rule: &Rule) -> String {
        let Rule::Text { ref contains } = *rule else {
            return String::new();
        };
        format!("Text contains \"{contains}\"")
    }

    fn sql(&self, rule: &Rule, base: i32) -> (String, Vec<Value>) {
        let Rule::Text { ref contains } = *rule else {
            return (String::new(), Vec::new());
        };
        let query = fts_query(contains);
        if query.is_empty() {
            return (String::new(), Vec::new());
        }
        (
            format!("p.id IN (SELECT rowid FROM photo_fts WHERE photo_fts MATCH ?{base})"),
            vec![Value::from(query)],
        )
    }
}

#[derive(Debug)]
pub struct RatingCriterion;
impl Criterion for RatingCriterion {
    fn id(&self) -> CriterionId {
        CriterionId("rating")
    }

    fn label(&self) -> &str {
        "Rating"
    }

    fn applies(&self, rule: &Rule) -> bool {
        matches!(rule, Rule::Rating { .. })
    }

    fn describe(&self, rule: &Rule) -> String {
        let Rule::Rating { op, stars } = *rule else {
            return String::new();
        };
        format!("Rating {} {}", op.sign(), stars)
    }

    fn sql(&self, rule: &Rule, base: i32) -> (String, Vec<Value>) {
        let Rule::Rating { op, stars } = *rule else {
            return (String::new(), Vec::new());
        };
        (
            format!("p.rating {} ?{base}", op.sign()),
            vec![Value::from(stars as i64)],
        )
    }
}

#[derive(Debug)]
pub struct FlagCriterion;
impl Criterion for FlagCriterion {
    fn id(&self) -> CriterionId {
        CriterionId("flag")
    }

    fn label(&self) -> &str {
        "Flag"
    }

    fn applies(&self, rule: &Rule) -> bool {
        matches!(rule, Rule::Flag { .. })
    }

    fn describe(&self, rule: &Rule) -> String {
        let Rule::Flag { value } = *rule else {
            return String::new();
        };
        let word = match value {
            FlagValue::Picked => "Picked",
            FlagValue::Rejected => "Rejected",
            FlagValue::Unflagged => "Unflagged",
        };
        format!("Flag is {word}")
    }

    fn sql(&self, rule: &Rule, base: i32) -> (String, Vec<Value>) {
        let Rule::Flag { value } = *rule else {
            return (String::new(), Vec::new());
        };
        (
            format!("p.flag = ?{base}"),
            vec![Value::from(value.as_i64())],
        )
    }
}

#[derive(Debug)]
pub struct LabelCriterion;
impl Criterion for LabelCriterion {
    fn id(&self) -> CriterionId {
        CriterionId("label")
    }

    fn label(&self) -> &str {
        "Color Label"
    }

    fn applies(&self, rule: &Rule) -> bool {
        matches!(rule, Rule::Label { .. })
    }

    fn describe(&self, rule: &Rule) -> String {
        let Rule::Label { ref label } = *rule else {
            return String::new();
        };
        match label {
            None => "Label is none".to_string(),
            Some(name) => format!("Label is {name}"),
        }
    }

    fn sql(&self, rule: &Rule, base: i32) -> (String, Vec<Value>) {
        let Rule::Label { ref label } = *rule else {
            return (String::new(), Vec::new());
        };
        match label {
            None => ("p.color_label IS NULL".to_string(), Vec::new()),
            Some(name) => (
                format!("p.color_label = ?{base}"),
                vec![Value::from(name.clone())],
            ),
        }
    }
}

#[derive(Debug)]
pub struct EditedCriterion;
impl Criterion for EditedCriterion {
    fn id(&self) -> CriterionId {
        CriterionId("edited")
    }

    fn label(&self) -> &str {
        "Edited"
    }

    fn applies(&self, rule: &Rule) -> bool {
        matches!(rule, Rule::Edited { .. })
    }

    fn describe(&self, rule: &Rule) -> String {
        let Rule::Edited { edited } = *rule else {
            return String::new();
        };
        if edited {
            "Edited".to_string()
        } else {
            "Not edited".to_string()
        }
    }

    fn sql(&self, rule: &Rule, base: i32) -> (String, Vec<Value>) {
        let Rule::Edited { edited } = *rule else {
            return (String::new(), Vec::new());
        };
        (
            format!(
                "EXISTS (SELECT 1 FROM develop_settings ds WHERE ds.photo_id = p.id) = ?{base}"
            ),
            vec![Value::from(edited)],
        )
    }
}

#[derive(Debug)]
pub struct VirtualCopyCriterion;
impl Criterion for VirtualCopyCriterion {
    fn id(&self) -> CriterionId {
        CriterionId("virtual_copy")
    }

    fn label(&self) -> &str {
        "Virtual Copy"
    }

    fn applies(&self, rule: &Rule) -> bool {
        matches!(rule, Rule::VirtualCopy { .. })
    }

    fn describe(&self, rule: &Rule) -> String {
        let Rule::VirtualCopy { is_copy } = *rule else {
            return String::new();
        };
        if is_copy {
            "Is a virtual copy".to_string()
        } else {
            "Is a master".to_string()
        }
    }

    fn sql(&self, rule: &Rule, base: i32) -> (String, Vec<Value>) {
        let Rule::VirtualCopy { is_copy } = *rule else {
            return (String::new(), Vec::new());
        };
        (
            format!("(p.copy_name IS NOT NULL) = ?{base}"),
            vec![Value::from(is_copy)],
        )
    }
}

/// One criterion for the three capture-date rules: `CaptureYear`,
/// `CaptureMonth` and `CaptureDay` — plan §7.5's date hierarchy, one
/// filter-bar column with three granularities, of which exactly one is
/// active at a time.
#[derive(Debug)]
pub struct CaptureDateCriterion;
impl Criterion for CaptureDateCriterion {
    fn id(&self) -> CriterionId {
        CriterionId("capture_date")
    }

    fn label(&self) -> &str {
        "Capture Date"
    }

    fn applies(&self, rule: &Rule) -> bool {
        matches!(
            rule,
            Rule::CaptureYear { .. } | Rule::CaptureMonth { .. } | Rule::CaptureDay { .. }
        )
    }

    fn describe(&self, rule: &Rule) -> String {
        match rule {
            Rule::CaptureYear { year } => format!("Captured in {}", year),
            Rule::CaptureMonth { year, month } => {
                format!("Captured in {}-{}", year, month_padded(*month))
            }
            Rule::CaptureDay { year, month, day } => {
                format!(
                    "Captured on {}-{}-{}",
                    year,
                    month_padded(*month),
                    day_padded(*day)
                )
            }
            _ => String::new(),
        }
    }

    fn sql(&self, rule: &Rule, base: i32) -> (String, Vec<Value>) {
        match rule {
            Rule::CaptureYear { year } => (
                format!("substr(f.capture_time, 1, 4) = ?{base}"),
                vec![Value::from(year.to_string())],
            ),
            Rule::CaptureMonth { year, month } => (
                format!("substr(f.capture_time, 1, 7) = ?{base}"),
                vec![Value::from(format!("{}-{}", year, month_padded(*month)))],
            ),
            Rule::CaptureDay { year, month, day } => (
                format!("substr(f.capture_time, 1, 10) = ?{base}"),
                vec![Value::from(format!(
                    "{}-{}-{}",
                    year,
                    month_padded(*month),
                    day_padded(*day)
                ))],
            ),
            _ => (String::new(), Vec::new()),
        }
    }
}

#[derive(Debug)]
pub struct CameraCriterion;
impl Criterion for CameraCriterion {
    fn id(&self) -> CriterionId {
        CriterionId("camera")
    }

    fn label(&self) -> &str {
        "Camera"
    }

    fn applies(&self, rule: &Rule) -> bool {
        matches!(rule, Rule::Camera { .. })
    }

    fn describe(&self, rule: &Rule) -> String {
        let Rule::Camera { ref model } = *rule else {
            return String::new();
        };
        format!("Camera is {model}")
    }

    fn sql(&self, rule: &Rule, base: i32) -> (String, Vec<Value>) {
        let Rule::Camera { ref model } = *rule else {
            return (String::new(), Vec::new());
        };
        (
            format!("f.camera_model = ?{base}"),
            vec![Value::from(model.clone())],
        )
    }
}

#[derive(Debug)]
pub struct LensCriterion;
impl Criterion for LensCriterion {
    fn id(&self) -> CriterionId {
        CriterionId("lens")
    }

    fn label(&self) -> &str {
        "Lens"
    }

    fn applies(&self, rule: &Rule) -> bool {
        matches!(rule, Rule::Lens { .. })
    }

    fn describe(&self, rule: &Rule) -> String {
        let Rule::Lens { ref contains } = *rule else {
            return String::new();
        };
        format!("Lens contains \"{contains}\"")
    }

    fn sql(&self, rule: &Rule, base: i32) -> (String, Vec<Value>) {
        let Rule::Lens { ref contains } = *rule else {
            return (String::new(), Vec::new());
        };
        // `!` is the escape character so `%`/`_` in the user's text lose
        // their wildcard meaning.
        let escaped = contains
            .replace("!", "!!")
            .replace("%", "!%")
            .replace("_", "!_");
        (
            format!("f.lens LIKE ?{base} ESCAPE '!'"),
            vec![Value::from(format!("%{escaped}%"))],
        )
    }
}

/// 1 → "01", 12 → "12" — the capture-time column stores ISO dates.
fn month_padded(month: u8) -> String {
    if month < 10 {
        format!("0{}", month)
    } else {
        month.to_string()
    }
}

/// 3 → "03", 31 → "31".
fn day_padded(day: u8) -> String {
    if day < 10 {
        format!("0{}", day)
    } else {
        day.to_string()
    }
}

// ---------------------------------------------------------------------
// Composition helpers
// ---------------------------------------------------------------------

/// Turns user filter text into an FTS5 query: each whitespace-separated
/// token becomes a quoted prefix match, ANDed together, so `sun beach`
/// finds photos whose FTS columns contain both tokens (plan §7.5's
/// "contains" semantics with the rows-match-all default). Embedded double
/// quotes are escaped per FTS5's quoted-string rule.
pub fn fts_query(text: &str) -> String {
    text.split(' ')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"*", t.replace("\"", "\"\"")))
        .collect::<Vec<String>>()
        .join(" AND ")
}

/// Combines `rules` into one parenthesized WHERE fragment and its
/// parameters. `match_all` chooses `AND` over `OR` between rules (the
/// smart-collection "match all/any" switch, plan §7.2); an empty rule
/// list yields the empty string. `base` is the first `?N` number to use —
/// a caller that already bound `?1` for its own source clause passes 2.
pub fn compose(rules: &[Rule], match_all: bool, base: i32) -> (String, Vec<Value>) {
    let mut fragments: Vec<String> = Vec::new();
    let mut params: Vec<Value> = Vec::new();
    for rule in rules {
        let (sql, values) = rule.sql(base + params.len() as i32);
        if !sql.is_empty() {
            fragments.push(sql);
            params.extend(values);
        }
    }
    if fragments.is_empty() {
        return (String::new(), Vec::new());
    }
    let joiner = if match_all { " AND " } else { " OR " };
    (format!("({})", fragments.join(joiner)), params)
}

/// `rules` minus every rule whose criterion id is in `excluded` — the
/// metadata-count queries use this so a column's open menu lists the
/// values left under the *other* filters only (Lightroom behaviour).
pub fn without_ids(rules: &[Rule], excluded: &[CriterionId]) -> Vec<Rule> {
    rules
        .iter()
        .filter(|rule| match rule_criterion_id(rule) {
            Some(id) => !excluded.contains(&id),
            None => true,
        })
        .cloned()
        .collect()
}

/// The one id for every capture-date granularity — the whole hierarchy is
/// a single filter-bar column, so choosing any level clears the others.
pub const CAPTURE_DATE_ID: CriterionId = CriterionId("capture_date");

/// A smart collection's saved definition (plan §7.2): the "match all/any"
/// switch plus its rule list, serialized into `collections.rules`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SmartRules {
    pub match_all: bool,
    pub rules: Vec<Rule>,
}

impl Default for SmartRules {
    fn default() -> Self {
        Self {
            match_all: true,
            rules: Vec::new(),
        }
    }
}

impl SmartRules {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    pub fn from_json(json: &str) -> Option<Self> {
        let parsed: Option<SmartRules> = serde_json::from_str(json).ok();
        parsed
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}
