// Shared reactive state, one class per domain (AGENTS.md: runes only, no extra stores).
// Anything that belongs to a single component stays in that component; these five are the
// domains more than one component reads. Commands follow `docs/rewrite-plan.md` §7.

import {
  appInfo,
  batchUpdate,
  beginGoogleAuth,
  cancelUpload,
  completeGoogleAuth,
  createFolder,
  deleteFiles,
  deleteFolder,
  disconnectAccount,
  getSettings,
  IpcError,
  listAccounts,
  listFiles,
  listFolders,
  moveFiles,
  moveFolder,
  onAccountsChanged,
  onSyncProgress,
  onUploadDone,
  onUploadProgress,
  recentFolders,
  renameFile,
  renameFolder,
  retryUpload,
  searchFiles,
  setGoogleClientId,
  setSettings,
  startUploadBatch,
  syncAccountQuota,
  syncGoogle,
  toggleArchive,
  toggleStar,
  trashFiles,
  type Account,
  type AccountQuota,
  type FileItem,
  type FilePatch,
  type FileQuery,
  type Folder,
  type SyncOutcome,
  type SyncProgress,
  type UploadProgress,
  type UploadTask,
  untrashFiles,
} from './ipc';

// Every wrapper already rejects with IpcError; this keeps the catch clause honest if one stops.
function failed(err: unknown): IpcError {
  return err instanceof IpcError ? err : new IpcError('unknown', String(err));
}

type UploadTaskState = UploadProgress['state'] | 'queued';

/** A queued file plus whatever the backend has told us about it so far. */
export interface UploadEntry {
  task: UploadTask;
  state: UploadTaskState;
  sent: number;
  /** `0` until routing has picked an account. */
  account_id: number;
}

class AccountsState {
  items = $state<Account[]>([]);
  quotas = $state<Record<number, AccountQuota>>({});
  /** `account_id` -> change entries applied so far; absent means that account is idle. */
  syncing = $state<Record<number, number>>({});
  loading = $state(false);
  connecting = $state(false);
  error = $state<IpcError | null>(null);

  limitBytes = $derived(Object.values(this.quotas).reduce((sum, quota) => sum + quota.limit_bytes, 0));
  usageBytes = $derived(Object.values(this.quotas).reduce((sum, quota) => sum + quota.usage_bytes, 0));
  isSyncing = $derived(Object.keys(this.syncing).length > 0);

  async load(): Promise<void> {
    this.loading = true;
    try {
      this.items = await listAccounts();
      this.error = null;
    } catch (err) {
      this.error = failed(err);
    } finally {
      this.loading = false;
    }
  }

  /**
   * Two commands by design: `begin` arms the loopback listener and points at the consent screen,
   * `complete` blocks until the browser comes back with a token.
   */
  async connect(): Promise<Account | null> {
    this.connecting = true;
    try {
      await beginGoogleAuth();
      const account = await completeGoogleAuth();
      this.error = null;
      await this.load();
      return account;
    } catch (err) {
      this.error = failed(err);
      return null;
    } finally {
      this.connecting = false;
    }
  }

  async disconnect(accountId: number): Promise<void> {
    try {
      await disconnectAccount(accountId);
      delete this.quotas[accountId];
      this.error = null;
      await this.load();
    } catch (err) {
      this.error = failed(err);
    }
  }

  async refreshQuota(accountId: number): Promise<void> {
    try {
      this.quotas[accountId] = await syncAccountQuota(accountId);
      this.error = null;
    } catch (err) {
      this.error = failed(err);
    }
  }

  async syncGoogle(): Promise<SyncOutcome[]> {
    try {
      const outcomes = await syncGoogle();
      this.error = null;
      return outcomes;
    } catch (err) {
      this.error = failed(err);
      return [];
    }
  }

  /** Wires `sync://progress` and `accounts://changed`; returns the stop for `$effect` cleanup. */
  async subscribe(): Promise<() => void> {
    stopAccounts?.();
    const [stopSync, stopList] = await Promise.all([
      onSyncProgress(mergeSync),
      onAccountsChanged((next) => {
        accounts.items = next;
      }),
    ]);
    stopAccounts = () => {
      stopSync();
      stopList();
    };
    return stopAccounts;
  }
}

class FoldersState {
  /** Navigation lives in the URL query; this mirrors it so the tree knows what is open. */
  currentFolderId = $state<number | null>(null);
  items = $state<Folder[]>([]);
  recent = $state<Folder[]>([]);
  loading = $state(false);
  error = $state<IpcError | null>(null);

  async load(parentId: number | null = this.currentFolderId): Promise<void> {
    this.loading = true;
    try {
      this.items = await listFolders(parentId);
      this.error = null;
    } catch (err) {
      this.error = failed(err);
    } finally {
      this.loading = false;
    }
  }

  async loadRecent(): Promise<void> {
    try {
      this.recent = await recentFolders();
      this.error = null;
    } catch (err) {
      this.error = failed(err);
    }
  }

  async create(name: string, parentId: number | null = this.currentFolderId): Promise<void> {
    try {
      await createFolder(name, parentId);
      this.error = null;
      await this.load(parentId);
    } catch (err) {
      this.error = failed(err);
    }
  }

  async rename(folderId: number, name: string): Promise<void> {
    try {
      await renameFolder(folderId, name);
      this.error = null;
      const folder = this.items.find((item) => item.id === folderId);
      if (folder) folder.name = name;
    } catch (err) {
      this.error = failed(err);
    }
  }

  async move(folderId: number, parentId: number | null): Promise<void> {
    try {
      await moveFolder(folderId, parentId);
      this.error = null;
      await this.load(parentId);
    } catch (err) {
      this.error = failed(err);
    }
  }

  async remove(folderId: number): Promise<void> {
    try {
      await deleteFolder(folderId);
      this.error = null;
      await this.load();
    } catch (err) {
      this.error = failed(err);
    }
  }
}

class FilesState {
  items = $state<FileItem[]>([]);
  /** The query that produced `items`; mutations reload with it instead of guessing. */
  query = $state<FileQuery>({});
  loading = $state(false);
  error = $state<IpcError | null>(null);

  async load(query: FileQuery): Promise<void> {
    this.loading = true;
    try {
      this.items = await listFiles(query);
      this.query = query;
      this.error = null;
    } catch (err) {
      this.error = failed(err);
    } finally {
      this.loading = false;
    }
  }

  /** A cleared box searches for nothing and lists everything again, like `list_files`. */
  async search(text: string, limit?: number): Promise<void> {
    this.loading = true;
    try {
      this.items = await searchFiles(text, limit);
      this.query = { search: text, limit };
      this.error = null;
    } catch (err) {
      this.error = failed(err);
    } finally {
      this.loading = false;
    }
  }

  async rename(fileId: number, name: string): Promise<void> {
    try {
      await renameFile(fileId, name);
      this.error = null;
      const file = this.items.find((item) => item.id === fileId);
      if (file) file.name = name;
    } catch (err) {
      this.error = failed(err);
    }
  }

  async move(fileIds: number[], folderId: number | null): Promise<void> {
    await this.reloadAfter(moveFiles(fileIds, folderId));
  }

  async remove(fileIds: number[]): Promise<void> {
    await this.reloadAfter(deleteFiles(fileIds));
  }

  async trash(fileIds: number[]): Promise<void> {
    await this.reloadAfter(trashFiles(fileIds));
  }

  async untrash(fileIds: number[]): Promise<void> {
    await this.reloadAfter(untrashFiles(fileIds));
  }

  /** Star and archive applied to a selection in one round trip. */
  async patch(fileIds: number[], patch: FilePatch): Promise<void> {
    try {
      await batchUpdate(fileIds, patch);
      this.error = null;
      for (const file of this.items) {
        if (!fileIds.includes(file.id)) continue;
        if (patch.starred !== undefined) file.starred = patch.starred;
        if (patch.archived !== undefined) file.archived = patch.archived;
      }
    } catch (err) {
      this.error = failed(err);
    }
  }

  /** The command reports the value it landed on, so one row patches without a refetch. */
  async toggleStar(fileId: number): Promise<void> {
    try {
      const starred = await toggleStar(fileId);
      this.error = null;
      const file = this.items.find((item) => item.id === fileId);
      if (file) file.starred = starred;
    } catch (err) {
      this.error = failed(err);
    }
  }

  async toggleArchive(fileId: number): Promise<void> {
    try {
      const archived = await toggleArchive(fileId);
      this.error = null;
      const file = this.items.find((item) => item.id === fileId);
      if (file) file.archived = archived;
    } catch (err) {
      this.error = failed(err);
    }
  }

  private async reloadAfter(pending: Promise<void>): Promise<void> {
    try {
      await pending;
      this.error = null;
      await this.load(this.query);
    } catch (err) {
      this.error = failed(err);
    }
  }
}

class UploadState {
  /** `task_id` -> entry; the only source is the enqueue reply and the two upload events. */
  tasks = $state<Record<number, UploadEntry>>({});
  error = $state<IpcError | null>(null);

  activeCount = $derived(
    Object.values(this.tasks).filter(
      (entry) => entry.state === 'started' || entry.state === 'running' || entry.state === 'paused',
    ).length,
  );

  async start(paths: string[], folderId: number | null): Promise<void> {
    try {
      seed(await startUploadBatch(paths, folderId));
      this.error = null;
    } catch (err) {
      this.error = failed(err);
    }
  }

  async cancel(taskId: number): Promise<void> {
    try {
      await cancelUpload(taskId);
      this.error = null;
      const entry = this.tasks[taskId];
      if (entry) entry.state = 'cancelled';
    } catch (err) {
      this.error = failed(err);
    }
  }

  async retry(taskId: number): Promise<void> {
    try {
      await retryUpload(taskId);
      this.error = null;
      const entry = this.tasks[taskId];
      if (entry) {
        entry.state = 'queued';
        entry.sent = 0;
        entry.account_id = 0;
      }
    } catch (err) {
      this.error = failed(err);
    }
  }

  /** Wires `upload://progress` and `upload://done`; returns the stop for `$effect` cleanup. */
  async subscribe(): Promise<() => void> {
    stopUpload?.();
    const [stopProgress, stopDone] = await Promise.all([onUploadProgress(mergeUpload), onUploadDone(mergeUpload)]);
    stopUpload = () => {
      stopProgress();
      stopDone();
    };
    return stopUpload;
  }
}

class SettingsState {
  values = $state<Record<string, string>>({});
  googleClientId = $state('');
  version = $state('');
  hasAccounts = $state(false);
  loading = $state(false);
  error = $state<IpcError | null>(null);

  async load(): Promise<void> {
    this.loading = true;
    try {
      this.values = await getSettings();
      this.error = null;
    } catch (err) {
      this.error = failed(err);
    } finally {
      this.loading = false;
    }
  }

  /** `set_settings` upserts, so the caller's view of the table wins for the keys it sends. */
  async save(values: Record<string, string>): Promise<void> {
    try {
      await setSettings(values);
      Object.assign(this.values, values);
      this.error = null;
    } catch (err) {
      this.error = failed(err);
    }
  }

  /** Own field rather than a guessed settings key: the command decides where it lands. */
  async saveGoogleClientId(clientId: string): Promise<void> {
    try {
      await setGoogleClientId(clientId);
      this.googleClientId = clientId;
      this.error = null;
    } catch (err) {
      this.error = failed(err);
    }
  }

  /** Boot check: one round trip decides the version and whether onboarding shows. */
  async refresh(): Promise<void> {
    this.loading = true;
    try {
      const info = await appInfo();
      this.version = info.version;
      this.hasAccounts = info.has_accounts;
      this.error = null;
    } catch (err) {
      this.error = failed(err);
    } finally {
      this.loading = false;
    }
  }
}

export const accounts = new AccountsState();
export const folders = new FoldersState();
export const files = new FilesState();
export const upload = new UploadState();
export const settings = new SettingsState();

let stopAccounts: (() => void) | null = null;
let stopUpload: (() => void) | null = null;

// A `started` tick can beat the enqueue reply, so park events for task ids we do not hold yet.
const pendingTicks: Record<number, UploadProgress> = {};

function apply(entry: UploadEntry, tick: UploadProgress): void {
  entry.state = tick.state;
  entry.sent = tick.sent;
  entry.account_id = tick.account_id;
}

function seed(tasks: UploadTask[]): void {
  for (const task of tasks) {
    const tick = pendingTicks[task.task_id];
    delete pendingTicks[task.task_id];
    const entry: UploadEntry = upload.tasks[task.task_id] ?? {
      task,
      state: 'queued',
      sent: 0,
      account_id: 0,
    };
    entry.task = task;
    if (tick) apply(entry, tick);
    upload.tasks[task.task_id] = entry;
  }
}

function mergeUpload(tick: UploadProgress): void {
  const entry = upload.tasks[tick.task_id];
  if (entry) apply(entry, tick);
  else pendingTicks[tick.task_id] = tick;
}

/** `restarted` means the page token expired and the run is going again, so it stays active. */
function mergeSync(tick: SyncProgress): void {
  if (tick.state === 'finished' || tick.state === 'failed') delete accounts.syncing[tick.account_id];
  else accounts.syncing[tick.account_id] = tick.processed;
}