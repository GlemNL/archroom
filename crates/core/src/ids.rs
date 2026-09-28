//! Newtype identifiers for catalog rows.
//!
//! Every ID wraps the `INTEGER PRIMARY KEY` rowid SQLite hands back on insert,
//! so the wrapper is a plain `i64` with no validation to do.

use std::fmt;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(
            Debug,
            Clone,
            Copy,
            Default,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            serde::Serialize,
            serde::Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub i64);

        impl $name {
            pub const fn new(id: i64) -> Self {
                Self(id)
            }

            pub const fn get(self) -> i64 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<i64> for $name {
            fn from(id: i64) -> Self {
                Self(id)
            }
        }
    };
}

id_type!(FolderId);
id_type!(FileId);
id_type!(PhotoId);
id_type!(ImportId);
id_type!(KeywordId);
id_type!(CollectionId);
id_type!(PresetId);
id_type!(HistoryId);
id_type!(SnapshotId);

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_through_json() {
        let id = PhotoId::new(42);
        let json = serde_json::to_string(&id).expect("serialize");
        assert_eq!(json, "42");
        let back: PhotoId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, id);
    }

    #[test]
    fn distinct_id_types_do_not_mix() {
        // This is a compile-time property, not a runtime one: PhotoId and
        // FileId are different types even though both wrap i64. The test
        // below just documents the intent.
        let photo = PhotoId::new(1);
        let file = FileId::new(1);
        assert_eq!(photo.get(), file.get());
    }
}
