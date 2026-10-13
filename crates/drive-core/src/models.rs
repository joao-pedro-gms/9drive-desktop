//! Row structs. `bool` on `starred`/`archived` is `INTEGER` 0/1 at the SQL boundary.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub id: i64,
    pub provider: String,
    pub name: String,
    pub email: String,
    pub external_account_id: Option<String>,
    pub drive_folder_id: Option<String>,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountQuota {
    pub account_id: i64,
    pub limit_bytes: i64,
    pub usage_bytes: i64,
    pub usage_in_drive: Option<i64>,
    pub usage_in_drive_trash: Option<i64>,
    pub synced_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileItem {
    pub id: i64,
    pub account_id: i64,
    pub drive_file_id: String,
    pub folder_id: Option<i64>,
    pub name: String,
    pub mime_type: Option<String>,
    pub size_bytes: Option<i64>,
    pub md5_checksum: Option<String>,
    pub starred: bool,
    pub archived: bool,
    pub trashed_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncState {
    pub account_id: i64,
    pub start_page_token: Option<String>,
    pub last_sync_at: Option<i64>,
}
