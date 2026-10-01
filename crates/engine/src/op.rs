//! The `Op` trait and the type-erased registry (plan §4.4). An op is
//! written once as a params struct plus metadata; persistence, grouping,
//! default UI and cache keys are derived from it. GPU kernels attach in
//! the pipeline (keyed by `Op::ID`), keeping this registry usable from a
//! CPU-only context such as the CLI or the catalog service.

use std::marker::PhantomData;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Pipeline stages (plan §6.2), in execution order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stage {
    Geometry = 1,
    Scene = 2,
    ToneMap = 3,
    Display = 4,
    Output = 5,
}

/// Granularity of presets, copy/paste and sync (plan §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsGroup {
    TreatmentProfile,
    WhiteBalance,
    BasicTone,
    Presence,
    ToneCurve,
    HslColor,
    BwMix,
    Sharpening,
    NoiseReduction,
    LensCorrections,
    Crop,
    Straighten,
    Effects,
    RedEye,
    LocalAdjustments,
    ProcessVersion,
}

impl SettingsGroup {
    /// Groups tied to one specific photo (circles and masks placed on it):
    /// presets never carry them and copy/paste leaves them unchecked.
    pub fn is_photo_specific(self) -> bool {
        matches!(
            self,
            SettingsGroup::RedEye | SettingsGroup::LocalAdjustments
        )
    }

    /// Every group, in panel order.
    pub const ALL: [SettingsGroup; 16] = [
        SettingsGroup::TreatmentProfile,
        SettingsGroup::WhiteBalance,
        SettingsGroup::BasicTone,
        SettingsGroup::Presence,
        SettingsGroup::ToneCurve,
        SettingsGroup::HslColor,
        SettingsGroup::BwMix,
        SettingsGroup::Sharpening,
        SettingsGroup::NoiseReduction,
        SettingsGroup::LensCorrections,
        SettingsGroup::Crop,
        SettingsGroup::Straighten,
        SettingsGroup::Effects,
        SettingsGroup::RedEye,
        SettingsGroup::LocalAdjustments,
        SettingsGroup::ProcessVersion,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SettingsGroup::TreatmentProfile => "Treatment & Profile",
            SettingsGroup::WhiteBalance => "White Balance",
            SettingsGroup::BasicTone => "Basic Tone",
            SettingsGroup::Presence => "Presence",
            SettingsGroup::ToneCurve => "Tone Curve",
            SettingsGroup::HslColor => "HSL / Color",
            SettingsGroup::BwMix => "B&W Mix",
            SettingsGroup::Sharpening => "Sharpening",
            SettingsGroup::NoiseReduction => "Noise Reduction",
            SettingsGroup::LensCorrections => "Lens Corrections",
            SettingsGroup::Crop => "Crop",
            SettingsGroup::Straighten => "Straighten",
            SettingsGroup::Effects => "Effects",
            SettingsGroup::RedEye => "Red Eye",
            SettingsGroup::LocalAdjustments => "Local Adjustments",
            SettingsGroup::ProcessVersion => "Process Version",
        }
    }
}

/// How a slider maps its value onto the track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Curve {
    Linear,
    /// Linear in mireds (1e6 / K), like Lightroom's Temp.
    Mired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Track {
    Plain,
    TempTint,
    Hue,
}

/// One scalar parameter, enough to build its slider (plan §4.4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamSpec {
    /// The key in the op's JSON object.
    pub key: &'static str,
    pub label: &'static str,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    pub fine: f64,
    pub coarse: f64,
    pub unit: &'static str,
    pub precision: u8,
    pub curve: Curve,
    pub track: Track,
}

impl ParamSpec {
    pub const fn linear(
        key: &'static str,
        label: &'static str,
        min: f64,
        max: f64,
        default: f64,
        fine: f64,
        coarse: f64,
    ) -> Self {
        Self {
            key,
            label,
            min,
            max,
            default,
            fine,
            coarse,
            unit: "",
            precision: 0,
            curve: Curve::Linear,
            track: Track::Plain,
        }
    }
}

/// One develop adjustment.
pub trait Op: Send + Sync + 'static {
    type Params: Serialize + DeserializeOwned + Default + Clone + PartialEq + Send + Sync;

    /// Stable key stored in catalogs.
    const ID: &'static str;
    /// Algorithm version → process versioning (plan §6.11).
    const VERSION: u32;
    const STAGE: Stage;
    /// Position inside the stage.
    const ORDER: u16;
    const GROUP: SettingsGroup;

    fn specs() -> &'static [ParamSpec];

    fn is_identity(p: &Self::Params) -> bool {
        *p == Self::Params::default()
    }
}

/// Type-erased view of an [`Op`]; params travel as JSON.
pub trait DynOp: Send + Sync {
    fn id(&self) -> &'static str;
    fn version(&self) -> u32;
    fn stage(&self) -> Stage;
    fn order(&self) -> u16;
    fn group(&self) -> SettingsGroup;
    fn specs(&self) -> &'static [ParamSpec];
    /// True when `params` (an op object, `v` ignored) equals the defaults.
    /// Unparseable params count as non-identity so they aren't dropped.
    fn is_identity(&self, params: &Value) -> bool;
    fn default_params(&self) -> Value;
}

#[derive(Debug)]
struct Entry<O: Op>(PhantomData<fn() -> O>);

impl<O: Op> DynOp for Entry<O> {
    fn id(&self) -> &'static str {
        O::ID
    }
    fn version(&self) -> u32 {
        O::VERSION
    }
    fn stage(&self) -> Stage {
        O::STAGE
    }
    fn order(&self) -> u16 {
        O::ORDER
    }
    fn group(&self) -> SettingsGroup {
        O::GROUP
    }
    fn specs(&self) -> &'static [ParamSpec] {
        O::specs()
    }
    fn is_identity(&self, params: &Value) -> bool {
        serde_json::from_value::<O::Params>(params.clone())
            .map(|p| O::is_identity(&p))
            .unwrap_or(false)
    }
    fn default_params(&self) -> Value {
        serde_json::to_value(O::Params::default()).unwrap_or(Value::Null)
    }
}

/// Every known op, sorted by (stage, order).
#[derive(Default)]
pub struct Registry {
    ops: Vec<Box<dyn DynOp>>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.ops.iter().map(|o| o.id()))
            .finish()
    }
}

impl Registry {
    pub fn register<O: Op>(&mut self) {
        self.ops.push(Box::new(Entry::<O>(PhantomData)));
        self.ops.sort_by_key(|o| (o.stage(), o.order()));
    }

    pub fn get(&self, id: &str) -> Option<&dyn DynOp> {
        self.ops.iter().find(|o| o.id() == id).map(|o| o.as_ref())
    }

    pub fn iter(&self) -> impl Iterator<Item = &dyn DynOp> {
        self.ops.iter().map(|o| o.as_ref())
    }
}
