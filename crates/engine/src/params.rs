//! `EditParams`: the persisted develop settings (plan §5.2). Only
//! non-identity ops are stored, each as a JSON object carrying its
//! algorithm version `v`; ops this build doesn't know survive a load/save
//! round trip untouched.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use xxhash_rust::xxh3::xxh3_128;

use crate::op::Op;

/// The process version new edits are made with (plan §6.11).
pub const CURRENT_PROCESS_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditParams {
    pub process_version: u32,
    #[serde(default)]
    pub ops: BTreeMap<String, Value>,
    /// Reserved for local adjustments (plan §6.12).
    #[serde(default)]
    pub local: Vec<Value>,
}

impl Default for EditParams {
    fn default() -> Self {
        Self {
            process_version: CURRENT_PROCESS_VERSION,
            ops: BTreeMap::new(),
            local: Vec::new(),
        }
    }
}

impl EditParams {
    /// The op's params, or its defaults when absent or unreadable.
    pub fn get<O: Op>(&self) -> O::Params {
        self.ops
            .get(O::ID)
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default()
    }

    /// Stores `p`, or removes the entry when it is the identity.
    pub fn set<O: Op>(&mut self, p: O::Params) {
        if O::is_identity(&p) {
            self.ops.remove(O::ID);
            return;
        }
        if let Ok(Value::Object(mut map)) = serde_json::to_value(&p) {
            map.insert("v".to_string(), Value::from(O::VERSION));
            self.ops.insert(O::ID.to_string(), Value::Object(map));
        }
    }

    /// The stored algorithm version of an op, if it has an entry.
    pub fn op_version(&self, id: &str) -> Option<u32> {
        self.ops.get(id)?.get("v")?.as_u64().map(|v| v as u32)
    }

    /// True when nothing would change the image.
    pub fn is_identity(&self) -> bool {
        self.ops.is_empty() && self.local.is_empty()
    }

    /// Canonical JSON: object keys are sorted (`serde_json` maps are
    /// ordered), so equal params always serialize to the same text.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// xxh3-128 of the canonical JSON (plan §5.2's `params_hash`).
    pub fn hash(&self) -> [u8; 16] {
        xxh3_128(self.to_json().as_bytes()).to_be_bytes()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::ops::{Exposure, ExposureParams};

    #[test]
    fn identity_ops_are_not_stored() {
        let mut p = EditParams::default();
        p.set::<Exposure>(ExposureParams { ev: 0.35 });
        assert_eq!(p.ops.len(), 1);
        assert_eq!(p.op_version("exposure"), Some(1));
        p.set::<Exposure>(ExposureParams { ev: 0.0 });
        assert!(p.is_identity());
    }

    #[test]
    fn json_round_trip_preserves_unknown_ops() {
        let json = r#"{"process_version":1,"ops":{"exposure":{"ev":0.5,"v":1},"future_op":{"v":9,"x":[1,2]}},"local":[]}"#;
        let p = EditParams::from_json(json).unwrap();
        assert_eq!(p.get::<Exposure>().ev, 0.5);
        let back = EditParams::from_json(&p.to_json()).unwrap();
        assert_eq!(back, p);
        assert!(back.ops.contains_key("future_op"));
    }

    #[test]
    fn hash_is_stable_across_key_order_and_sensitive_to_values() {
        let a =
            EditParams::from_json(r#"{"ops":{"exposure":{"v":1,"ev":0.5}},"process_version":1}"#)
                .unwrap();
        let b =
            EditParams::from_json(r#"{"process_version":1,"ops":{"exposure":{"ev":0.5,"v":1}}}"#)
                .unwrap();
        assert_eq!(a.hash(), b.hash());
        let mut c = a.clone();
        c.set::<Exposure>(ExposureParams { ev: 0.51 });
        assert_ne!(a.hash(), c.hash());
    }
}
