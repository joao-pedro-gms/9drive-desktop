// The only place allowed to touch Tauri's `invoke` / `listen` (AGENTS.md).
// The command surface is `docs/rewrite-plan.md` §7 verbatim; field names are snake_case because
// the Rust row structs derive plain `serde::Serialize` with no `rename_all`, so `drive_file_id`
// really is `drive_file_id` on the wire.

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export interface Account {
  id: number;
  provider: string;
  name: string;
  email: string;
  external_account_id: string | null;
  drive_folder_id: string | null;
  created_at: number;
  last_used_at: number | null;
}

export interface AccountQuota {
  account_id: number;
  limit_bytes: number;
  usage_bytes: number;
  usage_in_drive: number | null;
  usage_in_drive_trash: number | null;
  synced_at: number;
}

export interface Folder {
  id: number;
  parent_id: number | null;
  name: string;
  created_at: number;
  updated_at: number;
}

export interface FileItem {
  id: number;
  account_id: number;
  drive_file_id: string;
  folder_id: number | null;
  name: string;
  mime_type: string | null;
  size_bytes: number | null;
  md5_checksum: string | null;
  starred: boolean;
  archived: boolean;
  trashed_at: number | null;
  created_at: number;
  updated_at: number;
}

export interface UploadTask {
  task_id: number;
  source_path: string;
  name: string;
  mime_type: string;
  size_bytes: number;
  folder_id: number | null;
}

export interface SyncOutcome {
  processed: number;
  upserted: number;
  deleted: number;
  incremental: boolean;
}

export interface FileQuery {
  folder_id?: number | null;
  loose_only?: boolean;
  starred?: boolean;
  archived?: boolean;
  trashed?: boolean;
  search?: string;
  limit?: number;
}

export interface AppInfo {
  version: string;
  has_accounts: boolean;
}

/** What `begin_google_auth` hands back so the browser can be pointed at the consent screen. */
export interface AuthRequest {
  auth_url: string;
}

/** Byte window for `preview_data`; omit it for a whole file. */
export interface ByteRange {
  start: number;
  end: number;
}

/** The subset of flags `batch_update` can flip on a set of files. */
export interface FilePatch {
  starred?: boolean;
  archived?: boolean;
}

export type UploadState = 'started' | 'running' | 'finished' | 'paused' | 'cancelled' | 'failed';

export type SyncState = 'started' | 'running' | 'finished' | 'restarted' | 'failed';

/** `ProgressEvent` is an internally tagged enum; `kind` carries the channel it arrived on. */
export interface UploadProgress {
  kind: 'upload';
  task_id: number;
  name: string;
  /** `0` until routing has picked an account. */
  account_id: number;
  sent: number;
  total: number | null;
  state: UploadState;
}

export interface SyncProgress {
  kind: 'sync';
  account_id: number;
  processed: number;
  state: SyncState;
}

/** The `{ code, message }` every command rejects with; switch on `code`, never on `message`. */
export class IpcError extends Error {
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message);
    this.name = 'IpcError';
  }
}

function isIpcPayload(err: unknown): err is { code: string; message: string } {
  return (
    typeof err === 'object' &&
    err !== null &&
    'code' in err &&
    typeof err.code === 'string' &&
    'message' in err &&
    typeof err.message === 'string'
  );
}

function invokeCommand<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  return invoke<T>(command, args).catch((err: unknown) => {
    throw isIpcPayload(err) ? new IpcError(err.code, err.message) : new IpcError('unknown', String(err));
  });
}

// accounts
export const listAccounts = (): Promise<Account[]> => invokeCommand('list_accounts');
export const beginGoogleAuth = (): Promise<AuthRequest> => invokeCommand('begin_google_auth');
export const completeGoogleAuth = (): Promise<Account> => invokeCommand('complete_google_auth');
export const disconnectAccount = (account_id: number): Promise<void> =>
  invokeCommand('disconnect_account', { account_id });
export const syncAccountQuota = (account_id: number): Promise<AccountQuota> =>
  invokeCommand('sync_account_quota', { account_id });

// folders — the virtual tree is local SQLite only, it never touches Drive
export const listFolders = (parent_id: number | null): Promise<Folder[]> =>
  invokeCommand('list_folders', { parent_id });
export const createFolder = (name: string, parent_id: number | null): Promise<Folder> =>
  invokeCommand('create_folder', { name, parent_id });
export const renameFolder = (folder_id: number, name: string): Promise<void> =>
  invokeCommand('rename_folder', { folder_id, name });
export const moveFolder = (folder_id: number, parent_id: number | null): Promise<void> =>
  invokeCommand('move_folder', { folder_id, parent_id });
export const deleteFolder = (folder_id: number): Promise<void> =>
  invokeCommand('delete_folder', { folder_id });
export const recentFolders = (): Promise<Folder[]> => invokeCommand('recent_folders');

// files
export const listFiles = (query: FileQuery): Promise<FileItem[]> => invokeCommand('list_files', { query });
export const getFile = (file_id: number): Promise<FileItem> => invokeCommand('get_file', { file_id });
export const searchFiles = (query: string, limit?: number): Promise<FileItem[]> =>
  invokeCommand('search_files', { query, limit });
export const renameFile = (file_id: number, name: string): Promise<void> =>
  invokeCommand('rename_file', { file_id, name });
export const moveFiles = (file_ids: number[], folder_id: number | null): Promise<void> =>
  invokeCommand('move_files', { file_ids, folder_id });
export const batchUpdate = (file_ids: number[], patch: FilePatch): Promise<void> =>
  invokeCommand('batch_update', { file_ids, patch });
export const deleteFiles = (file_ids: number[]): Promise<void> =>
  invokeCommand('delete_files', { file_ids });
/** Rows get a `trashed_at`, so the current listing has to be refetched afterwards. */
export const trashFiles = (file_ids: number[]): Promise<void> => invokeCommand('trash_files', { file_ids });
export const untrashFiles = (file_ids: number[]): Promise<void> => invokeCommand('untrash_files', { file_ids });
/** Rust flips the flag and reports the value it landed on, so the row can patch without a refetch. */
export const toggleStar = (file_id: number): Promise<boolean> => invokeCommand('toggle_star', { file_id });
export const toggleArchive = (file_id: number): Promise<boolean> => invokeCommand('toggle_archive', { file_id });
export const syncGoogle = (): Promise<SyncOutcome[]> => invokeCommand('sync_google');

// upload
export const startUploadBatch = (paths: string[], folder_id: number | null): Promise<UploadTask[]> =>
  invokeCommand('start_upload_batch', { paths, folder_id });
export const cancelUpload = (task_id: number): Promise<void> => invokeCommand('cancel_upload', { task_id });
export const retryUpload = (task_id: number): Promise<void> => invokeCommand('retry_upload', { task_id });

// preview / download — bytes for a blob URL; large media streams over `9drive://` instead
export const previewData = (file_id: number, range?: ByteRange): Promise<number[]> =>
  invokeCommand('preview_data', { file_id, range });
/** Resolves to the chosen path, or `null` when the user dismissed the save dialog. */
export const saveFileAs = (file_id: number): Promise<string | null> => invokeCommand('save_file_as', { file_id });

// settings
export const getSettings = (): Promise<Record<string, string>> => invokeCommand('get_settings');
export const setSettings = (values: Record<string, string>): Promise<void> =>
  invokeCommand('set_settings', { values });
export const setGoogleClientId = (client_id: string): Promise<void> =>
  invokeCommand('set_google_client_id', { client_id });
export const openAppDir = (): Promise<void> => invokeCommand('open_app_dir');
export const appInfo = (): Promise<AppInfo> => invokeCommand('app_info');

// events — `docs/rewrite-plan.md` §7: progress is events, never polling
export const onUploadProgress = (cb: (progress: UploadProgress) => void): Promise<UnlistenFn> =>
  listen<UploadProgress>('upload://progress', (event) => cb(event.payload));

/** Same payload as `upload://progress`, terminal states only. */
export const onUploadDone = (cb: (progress: UploadProgress) => void): Promise<UnlistenFn> =>
  listen<UploadProgress>('upload://done', (event) => cb(event.payload));

export const onSyncProgress = (cb: (progress: SyncProgress) => void): Promise<UnlistenFn> =>
  listen<SyncProgress>('sync://progress', (event) => cb(event.payload));

/** The full account list after any connect, disconnect or quota-changing sync. */
export const onAccountsChanged = (cb: (accounts: Account[]) => void): Promise<UnlistenFn> =>
  listen<Account[]>('accounts://changed', (event) => cb(event.payload));