//! One place where a Drive v3 `files` resource becomes a [`RemoteFile`].
//!
//! Shared by every endpoint so the three sibling lanes cannot drift apart on which Drive
//! fields are requested or on how Drive's spellings differ from ours.

use serde::Deserialize;

use crate::types::RemoteFile;

/// The subset of the Drive v3 `File` resource the provider asks for. Every field is optional
/// except `id` because `changes.list` returns a bare `{"fileId": "..."}`-style resource for
/// changes that are not content changes.
#[derive(Debug, Deserialize)]
pub(crate) struct DriveFile {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(rename = "mimeType", default)]
    pub mime_type: String,
    /// A JSON **string**, not a number. Workspace documents omit it entirely.
    #[serde(rename = "sizeBytes")]
    pub size_bytes: Option<String>,
    #[serde(rename = "md5Checksum")]
    pub md5_checksum: Option<String>,
    #[serde(default)]
    pub parents: Vec<String>,
    #[serde(default)]
    pub trashed: bool,
    /// Kept verbatim as RFC-3339: parsing it here would bake one provider's timezone
    /// conventions into every consumer.
    #[serde(rename = "modifiedTime")]
    pub modified_at: Option<String>,
    pub capabilities: Option<Capabilities>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Capabilities {
    #[serde(rename = "canDownload", default)]
    can_download: bool,
}

/// `fields=` asking Drive for exactly what [`remote_file`] reads. Without `capabilities` in the
/// field mask Drive omits it and `can_download` would silently read as false for every file.
pub(crate) const FILE_FIELDS: &str = "id,name,mimeType,sizeBytes,md5Checksum,parents,trashed,\
modifiedTime,capabilities(canDownload)";

pub(crate) fn remote_file(file: &DriveFile) -> RemoteFile {
    RemoteFile {
        id: file.id.clone(),
        name: file.name.clone().unwrap_or_default(),
        mime_type: file.mime_type.clone(),
        size_bytes: file.size_bytes.as_deref().and_then(|raw| raw.parse().ok()),
        md5_checksum: file.md5_checksum.clone(),
        parent_id: file.parents.first().cloned(),
        trashed: file.trashed,
        modified_at: file.modified_at.clone(),
        can_download: file.capabilities.as_ref().is_some_and(|caps| caps.can_download),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &str) -> DriveFile {
        serde_json::from_str(raw).expect("drive file")
    }

    #[test]
    fn a_size_is_a_string_in_drive_and_a_number_here() {
        let mapped = remote_file(&parse(r#"{"id":"1","sizeBytes":"123456"}"#));
        assert_eq!(mapped.size_bytes, Some(123_456));
    }

    #[test]
    fn a_workspace_document_has_no_size_and_cannot_be_downloaded() {
        let mapped =
            remote_file(&parse(r#"{"id":"1","mimeType":"application/vnd.google-apps.document"}"#));
        assert_eq!(mapped.size_bytes, None);
        assert!(!mapped.can_download);
    }

    #[test]
    fn can_download_follows_the_capability_when_drive_sends_it() {
        assert!(
            remote_file(&parse(r#"{"id":"1","capabilities":{"canDownload":true}}"#)).can_download
        );
        assert!(
            !remote_file(&parse(r#"{"id":"1","capabilities":{"canDownload":false}}"#)).can_download
        );
        // Requested but absent means Drive did not authorise it, not that it is fine.
        assert!(!remote_file(&parse(r#"{"id":"1","capabilities":{}}"#)).can_download);
    }

    #[test]
    fn a_bare_change_resource_parses_without_inventing_data() {
        let mapped = remote_file(&parse(r#"{"id":"1"}"#));
        assert_eq!(mapped.name, "");
        assert_eq!(mapped.parent_id, None);
        assert_eq!(mapped.modified_at, None);
    }

    #[test]
    fn the_first_parent_wins_and_a_shared_folder_takes_the_first_listed() {
        let mapped = remote_file(&parse(r#"{"id":"1","parents":["first","second"]}"#));
        assert_eq!(mapped.parent_id.as_deref(), Some("first"));
        assert_eq!(remote_file(&parse(r#"{"id":"1"}"#)).parent_id, None);
    }

    #[test]
    fn the_field_mask_asks_for_everything_remote_file_reads() {
        for field in [
            "id",
            "name",
            "mimeType",
            "sizeBytes",
            "md5Checksum",
            "parents",
            "trashed",
            "modifiedTime",
            "capabilities(canDownload)",
        ] {
            assert!(FILE_FIELDS.contains(field), "{field} missing from {FILE_FIELDS}");
        }
    }
}
