//! Typed identifiers so mod, profile and group ids cannot be mixed up.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! string_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }
    };
}

string_id!(
    /// `gb<submission>`, `local_<12 hex>`, or `merged_<unix_ms>`.
    ModId
);
string_id!(
    /// `default` or `p_<12 hex>`.
    ProfileId
);
string_id!(
    /// `g_<12 hex>`.
    GroupId
);
string_id!(
    /// First 12 hex chars of sha1(lowercased canonical game path).
    GameId
);
string_id!(
    /// 64 lowercase hex chars of the sha256 of a file's content.
    BlobHash
);

impl ModId {
    /// GameBanana mod id for a submission.
    pub fn gamebanana(submission: u64) -> Self {
        Self(format!("gb{submission}"))
    }

    /// The GameBanana submission id encoded in a `gb<id>` mod id.
    pub fn gamebanana_submission(&self) -> Option<u64> {
        self.0.strip_prefix("gb").and_then(|rest| rest.parse().ok())
    }

    /// Mod ids are used in file names; only `[A-Za-z0-9_-]` is allowed.
    pub fn is_valid(&self) -> bool {
        !self.0.is_empty()
            && self.0.len() <= 64
            && self
                .0
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    }
}

impl ProfileId {
    pub fn default_profile() -> Self {
        Self("default".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gamebanana_round_trip() {
        let id = ModId::gamebanana(123);
        assert_eq!(id.as_str(), "gb123");
        assert_eq!(id.gamebanana_submission(), Some(123));
        assert_eq!(ModId::from("local_abc").gamebanana_submission(), None);
    }

    #[test]
    fn validity() {
        assert!(ModId::from("gb1").is_valid());
        assert!(ModId::from("local_0a1b2c3d4e5f").is_valid());
        assert!(!ModId::from("").is_valid());
        assert!(!ModId::from("../x").is_valid());
        assert!(!ModId::from("a b").is_valid());
    }

    #[test]
    fn serializes_transparently() {
        let json = serde_json::to_string(&ModId::from("gb5")).unwrap();
        assert_eq!(json, "\"gb5\"");
    }
}
