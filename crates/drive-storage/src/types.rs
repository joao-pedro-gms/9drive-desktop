//! Plain data crossing the [`crate::StorageProvider`] boundary.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{ProviderError, Result};

/// Remote backend a provider talks to. `#[non_exhaustive]` keeps adding one out of the set of
/// breaking changes for anyone matching on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Google,
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Google => "google",
        };
        f.write_str(name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountInfo {
    pub email: String,
    pub name: Option<String>,
    pub account_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quota {
    pub limit_bytes: u64,
    pub usage_bytes: u64,
    pub usage_in_drive: Option<u64>,
    pub usage_in_drive_trash: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteFile {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub size_bytes: Option<u64>,
    pub md5_checksum: Option<String>,
    pub parent_id: Option<String>,
    pub trashed: bool,
    /// Provider's own RFC-3339 timestamp, kept verbatim: parsing it here would bake one
    /// provider's precision and offset conventions into every consumer.
    pub modified_at: Option<String>,
    /// False for Workspace documents, which have to be exported instead of downloaded.
    pub can_download: bool,
}

/// One page of a folder listing. Providers page instead of truncating so the caller still sees
/// every entry by following `next_page_token`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderListing {
    pub entries: Vec<RemoteFile>,
    pub next_page_token: Option<String>,
}

/// One page of the incremental change feed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangePage {
    pub changes: Vec<RemoteFile>,
    pub deleted_ids: Vec<String>,
    /// Paging within the current run. `None` on the last page, where `new_start_page_token`
    /// is the one that matters.
    pub next_page_token: Option<String>,
    pub has_more: bool,
    /// Token to store and pass to the next `changes` call once this run finishes. Drive only
    /// returns it on the final page, and it is the only way to resume the feed where this run
    /// stopped instead of replaying the whole window.
    pub new_start_page_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadRequest {
    pub parent_id: String,
    pub name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    /// Local filesystem path. The provider streams from disk in chunks and must never buffer
    /// the whole file in memory (AGENTS.md security rules).
    pub source_path: String,
}

/// Inclusive byte range, as the HTTP `Range` header spells it: `end` is a position, not a length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Range {
    start: u64,
    end: u64,
}

impl Range {
    /// `end` is inclusive. `end < start` is a caller bug, not a provider failure, so it is
    /// reported before any request is attempted.
    pub fn new(start: u64, end: u64) -> Result<Self> {
        if end < start {
            return Err(ProviderError::InvalidArgument(format!("invalid range {start}-{end}")));
        }
        Ok(Self { start, end })
    }

    pub fn end(&self) -> u64 {
        self.end
    }

    pub fn start(&self) -> u64 {
        self.start
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::value::{Error as ValueError, StrDeserializer};

    #[test]
    fn range_rejects_end_before_start_and_keeps_an_inclusive_end() {
        assert_eq!(Range::new(10, 9).unwrap_err().code(), "invalid_argument");

        let range = Range::new(0, 1023).unwrap();
        assert_eq!(range.end(), 1023);
        assert_eq!(range, Range { start: 0, end: 1023 });
        // A single byte is start == end, not a zero-length range.
        assert_eq!(Range::new(7, 7).unwrap().end(), 7);
    }

    #[test]
    fn provider_kind_wire_string_is_lowercase_and_round_trips() {
        assert_eq!(ProviderKind::Google.to_string(), "google");

        let wire = ProviderKind::Google.serialize(WireWord).unwrap();
        assert_eq!(wire, "google");
        // Back through serde: `serde_json::from_str("\"google\"")` and the SQLite `provider`
        // column both hand the bare word to this same derived visitor.
        let back = ProviderKind::deserialize(StrDeserializer::<ValueError>::new(&wire)).unwrap();
        assert_eq!(back, ProviderKind::Google);
    }

    // Any shape other than a fieldless unit variant means this test-only helper would silently
    // stop asserting the wire format.
    macro_rules! not_a_unit_variant {
        ($name:ident ($($params:tt)*) -> $ret:ty) => {
            fn $name($($params)*) -> std::result::Result<$ret, std::fmt::Error> {
                panic!("ProviderKind must be a unit variant, not `{}`", stringify!($($params)*))
            }
        };
    }

    /// Captures the name serde emits for a unit variant. Hand-rolled because the crate takes no
    /// `serde_json` dependency and its tests are the only ones that need a serializer at all.
    struct WireWord;

    impl serde::Serializer for WireWord {
        type Ok = String;
        type Error = std::fmt::Error;
        type SerializeSeq = serde::ser::Impossible<String, Self::Error>;
        type SerializeTuple = serde::ser::Impossible<String, Self::Error>;
        type SerializeTupleStruct = serde::ser::Impossible<String, Self::Error>;
        type SerializeTupleVariant = serde::ser::Impossible<String, Self::Error>;
        type SerializeMap = serde::ser::Impossible<String, Self::Error>;
        type SerializeStruct = serde::ser::Impossible<String, Self::Error>;
        type SerializeStructVariant = serde::ser::Impossible<String, Self::Error>;

        fn serialize_unit_variant(
            self,
            _name: &'static str,
            _index: u32,
            variant: &'static str,
        ) -> std::result::Result<Self::Ok, Self::Error> {
            Ok(variant.to_string())
        }

        not_a_unit_variant!(serialize_bool(self, _v: bool) -> String);
        not_a_unit_variant!(serialize_i8(self, _v: i8) -> String);
        not_a_unit_variant!(serialize_i16(self, _v: i16) -> String);
        not_a_unit_variant!(serialize_i32(self, _v: i32) -> String);
        not_a_unit_variant!(serialize_i64(self, _v: i64) -> String);
        not_a_unit_variant!(serialize_u8(self, _v: u8) -> String);
        not_a_unit_variant!(serialize_u16(self, _v: u16) -> String);
        not_a_unit_variant!(serialize_u32(self, _v: u32) -> String);
        not_a_unit_variant!(serialize_u64(self, _v: u64) -> String);
        not_a_unit_variant!(serialize_f32(self, _v: f32) -> String);
        not_a_unit_variant!(serialize_f64(self, _v: f64) -> String);
        not_a_unit_variant!(serialize_char(self, _v: char) -> String);
        not_a_unit_variant!(serialize_str(self, _v: &str) -> String);
        not_a_unit_variant!(serialize_bytes(self, _v: &[u8]) -> String);
        not_a_unit_variant!(serialize_none(self) -> String);
        // The macro cannot match a generic parameter list, so these three are spelled out.
        fn serialize_some<T: ?Sized + Serialize>(
            self,
            _value: &T,
        ) -> std::result::Result<Self::Ok, Self::Error> {
            Err(std::fmt::Error)
        }
        fn serialize_newtype_struct<T: ?Sized + Serialize>(
            self,
            _name: &'static str,
            _value: &T,
        ) -> std::result::Result<Self::Ok, Self::Error> {
            Err(std::fmt::Error)
        }
        fn serialize_newtype_variant<T: ?Sized + Serialize>(
            self,
            _name: &'static str,
            _index: u32,
            _variant: &'static str,
            _value: &T,
        ) -> std::result::Result<Self::Ok, Self::Error> {
            Err(std::fmt::Error)
        }
        not_a_unit_variant!(serialize_unit(self) -> String);
        not_a_unit_variant!(serialize_unit_struct(self, _name: &'static str) -> String);
        not_a_unit_variant!(serialize_seq(self, _len: Option<usize>) -> Self::SerializeSeq);
        not_a_unit_variant!(serialize_tuple(self, _len: usize) -> Self::SerializeTuple);
        not_a_unit_variant!(serialize_tuple_struct(
            self,
            _name: &'static str,
            _len: usize
        ) -> Self::SerializeTupleStruct);
        not_a_unit_variant!(serialize_tuple_variant(
            self,
            _name: &'static str,
            _index: u32,
            _variant: &'static str,
            _len: usize
        ) -> Self::SerializeTupleVariant);
        not_a_unit_variant!(serialize_map(self, _len: Option<usize>) -> Self::SerializeMap);
        not_a_unit_variant!(serialize_struct(
            self,
            _name: &'static str,
            _len: usize
        ) -> Self::SerializeStruct);
        not_a_unit_variant!(serialize_struct_variant(
            self,
            _name: &'static str,
            _index: u32,
            _variant: &'static str,
            _len: usize
        ) -> Self::SerializeStructVariant);
    }
}
