<script lang="ts">
  // The file browser. It owns the things that are true of a listing rather than of a row:
  // which view is open, the sort, the selection, and every mutation that acts on it.
  //
  // `files` / `folders` are the shared state classes, so this screen is the one place that knows
  // both. It does not own the data: the shell reloads the listing on every `onchanged()`, and this
  // screen reloads the child folders whenever it enters one.
  import {
    Archive,
    Download,
    Eye,
    FolderOpen,
    FolderSymlink,
    Info,
    PenLine,
    Star,
    Trash2,
  } from '../lib/icons';
  import { files, folders } from '../lib/state.svelte';
  import { nav } from '../lib/nav.svelte';
  import { saveFileAs, type FileItem, type Folder } from '../lib/ipc';
  import type { IconComponent } from '../lib/icons';
  import FileGrid from '../lib/components/drive/FileGrid.svelte';
  import FileTable, {
    compareFiles,
    type Sort,
    type SortKey,
  } from '../lib/components/drive/FileTable.svelte';
  import FolderDialog from '../lib/components/drive/FolderDialog.svelte';
  import FolderPickerDialog from '../lib/components/drive/FolderPickerDialog.svelte';
  import RenameDialog from '../lib/components/drive/RenameDialog.svelte';
  import SelectionBar from '../lib/components/drive/SelectionBar.svelte';
  import DetailsPanel from '../lib/components/drive/DetailsPanel.svelte';
  import ContextMenu, { type MenuEntry } from '../lib/components/ui/ContextMenu.svelte';
  import Button from '../lib/components/ui/Button.svelte';
  import Dialog from '../lib/components/ui/Dialog.svelte';
  import EmptyState from '../lib/components/ui/EmptyState.svelte';
  import ErrorBanner from '../lib/components/ui/ErrorBanner.svelte';

  type FileView = 'files' | 'starred' | 'archived' | 'trash';

  interface Props {
    view: FileView;
    /** Only meaningful for `files`; the other views are flat. */
    folderId: number | null;
    /** The shell reloads the listing and the folder tree after any mutation. */
    onchanged: () => void;
  }

  let { view, folderId, onchanged }: Props = $props();

  // Folders only exist in the file view; the other three list files across every folder.
  let selected = $state<Set<number>>(new Set());
  /** Last row the user clicked, so Shift-click knows where the range starts. */
  let anchor = $state<number | null>(null);
  let sort = $state<Sort>({ key: 'name', dir: 'asc' });
  let menu = $state<{ open: boolean; x: number; y: number; entries: MenuEntry[] }>({
    open: false,
    x: 0,
    y: 0,
    entries: [],
  });
  let renameDialog = $state<{ open: boolean; file: FileItem | null }>({ open: false, file: null });
  let moveDialog = $state<{ open: boolean; folder: Folder | null }>({ open: false, folder: null });
  let folderDialog = $state<{ open: boolean; folder: Folder | null }>({ open: false, folder: null });
  let confirmDelete = $state<{ open: boolean; ids: number[] }>({ open: false, ids: [] });
  let detailsFor = $state<FileItem | null>(null);
  let deleting = $state(false);

  const childFolders = $derived(view === 'files' ? folders.items : []);
  const inTrash = $derived(view === 'trash');

  const sortedFolders = $derived(
    [...childFolders].sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true })),
  );

  /** Trash is a timeline: the newest deletion first is the only useful order there. */
  $effect(() => {
    sort = view === 'trash' ? { key: 'date', dir: 'desc' } : { key: 'name', dir: 'asc' };
  });

  const sortedFiles = $derived([...files.items].sort((a, b) => compareFiles(a, b, sort)));

  const selectedIds = $derived([...selected]);

  const allStarred = $derived(
    selectedIds.length > 0 && sortedFiles.filter((file) => selected.has(file.id)).every((file) => file.starred),
  );

  const allArchived = $derived(
    selectedIds.length > 0 && sortedFiles.filter((file) => selected.has(file.id)).every((file) => file.archived),
  );

  $effect(() => {
    if (view === 'files') void folders.load(folderId);
  });

  // A refetch after a mutation changes the id set; a selection that points at rows that are gone
  // would otherwise keep acting on files the user can no longer see.
  $effect(() => {
    const live = new Set(files.items.map((file) => file.id));
    if (selected.size === 0) return;
    const next = new Set([...selected].filter((id) => live.has(id)));
    if (next.size !== selected.size) selected = next;
  });

  const overlayOpen = $derived(
    menu.open ||
      renameDialog.open ||
      moveDialog.open ||
      folderDialog.open ||
      confirmDelete.open ||
      detailsFor !== null,
  );

  const empty = $derived(files.items.length === 0 && childFolders.length === 0);

  const emptyView = $derived.by((): { icon: IconComponent; title: string; hint: string } => {
    const search = files.query.search;
    if (search) return { icon: FolderOpen, title: 'No matches', hint: `Nothing matches “${search}”` };
    if (view === 'starred') return { icon: Star, title: 'No starred files', hint: 'Star a file to find it here' };
    if (view === 'archived')
      return { icon: Archive, title: 'Nothing archived', hint: 'Archived files stay out of your folders' };
    if (view === 'trash')
      return { icon: Trash2, title: 'Trash is empty', hint: 'Deleted files stay here until you remove them' };
    return { icon: FolderOpen, title: 'This folder is empty', hint: 'Drop files here, or use Upload' };
  });

  const scope = $derived.by(() => {
    const folderWord = childFolders.length === 1 ? 'folder' : 'folders';
    const fileWord = files.items.length === 1 ? 'file' : 'files';
    if (files.loading) return 'Loading…';
    if (view === 'files')
      return `${childFolders.length} ${folderWord} · ${files.items.length} ${fileWord}`;
    if (view === 'starred') return 'Files you starred';
    if (view === 'archived') return 'Files you archived';
    return `${files.items.length} deleted ${fileWord}`;
  });

  function toggleSort(key: SortKey): void {
    sort =
      sort.key === key
        ? { key, dir: sort.dir === 'asc' ? 'desc' : 'asc' }
        : { key, dir: key === 'name' ? 'asc' : 'desc' };
  }

  function clearSelection(): void {
    if (selected.size === 0) return;
    selected = new Set();
    anchor = null;
  }

  function selectFile(event: MouseEvent, file: FileItem): void {
    const ids = sortedFiles.map((item) => item.id);
    if (event.shiftKey && anchor !== null) {
      const from = ids.indexOf(anchor);
      const to = ids.indexOf(file.id);
      if (from !== -1 && to !== -1) {
        const [lo, hi] = from <= to ? [from, to] : [to, from];
        selected = new Set([...selected, ...ids.slice(lo, hi + 1)]);
        return;
      }
    }
    if (event.ctrlKey || event.metaKey) {
      const next = new Set(selected);
      if (next.has(file.id)) next.delete(file.id);
      else next.add(file.id);
      selected = next;
      anchor = file.id;
      return;
    }
    selected = new Set([file.id]);
    anchor = file.id;
  }

  function selectAll(): void {
    selected = new Set(sortedFiles.map((file) => file.id));
  }

  /**
   * The selection strip's whole prop set as one object: eight callbacks over the same selection,
   * and inlining them in the template buries them under more markup than logic.
   */
  const bar = $derived({
    count: selected.size,
    inTrash,
    allStarred,
    allArchived,
    onstar: () => applyToSelection((ids) => files.patch(ids, { starred: !allStarred })),
    onarchive: () => applyToSelection((ids) => files.patch(ids, { archived: !allArchived })),
    onmove: () => (moveDialog = { open: true, folder: null }),
    ontrash: () => applyToSelection((ids) => files.trash(ids)),
    onrestore: () => applyToSelection((ids) => files.untrash(ids)),
    ondelete: () => (confirmDelete = { open: true, ids: selectedIds }),
    ondownload: () => void download(selectedIds),
    oncancel: clearSelection,
  });

  /** Capture the ids first: the selection is cleared the moment the write is queued. */
  function applyToSelection(action: (ids: number[]) => Promise<void>): void {
    const ids = selectedIds;
    clearSelection();
    void mutate(ids, action);
  }

  function onkeydown(event: KeyboardEvent): void {
    const target = event.target;
    if (target instanceof HTMLElement && (target.isContentEditable || target.tagName === 'INPUT')) return;
    if (event.key === 'Escape' && !overlayOpen) {
      clearSelection();
      return;
    }
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'a') {
      event.preventDefault();
      selectAll();
    }
  }

  /** Every action goes through here so `onchanged` is never forgotten and the list refetches. */
  async function mutate(ids: number[], action: (ids: number[]) => Promise<void>): Promise<void> {
    if (ids.length === 0) return;
    await action(ids);
    onchanged();
  }

  async function mutateFolder(action: () => Promise<void>): Promise<void> {
    await action();
    await folders.load(folderId);
    onchanged();
  }

  /** `FolderDialog` did its own write; the screen only has to catch the listings up. */
  function folderChanged(): void {
    void folders.load(folderId);
    onchanged();
  }

  async function download(ids: number[]): Promise<void> {
    for (const id of ids) await saveFileAs(id);
  }

  /** A row action covers the whole selection when the row is part of it, and just the row if not. */
  function targetsFor(file: FileItem): number[] {
    return selected.has(file.id) ? selectedIds : [file.id];
  }

  function openMenu(event: MouseEvent, entries: MenuEntry[]): void {
    event.preventDefault();
    event.stopPropagation();
    menu = { open: true, x: event.clientX, y: event.clientY, entries };
  }

  function fileEntries(file: FileItem): MenuEntry[] {
    const ids = targetsFor(file);
    const many = ids.length > 1;
    const entries: MenuEntry[] = [
      { label: 'Open', icon: Eye, onclick: () => nav.setFile(file.id) },
      {
        label: many ? `Download ${ids.length} files` : 'Download',
        icon: Download,
        onclick: () => void download(ids),
      },
      {
        label: 'Rename',
        icon: PenLine,
        // A trashed file has no Drive name to change until it is restored.
        disabled: inTrash,
        onclick: () => (renameDialog = { open: true, file }),
      },
    ];

    if (inTrash) {
      entries.push(
        {
          label: 'Restore',
          icon: FolderSymlink,
          onclick: () => void mutate(ids, (list) => files.untrash(list)),
        },
        {
          label: 'Delete forever',
          icon: Trash2,
          danger: true,
          onclick: () => (confirmDelete = { open: true, ids }),
        },
      );
    } else {
      entries.push(
        {
          label: file.starred ? 'Unstar' : 'Star',
          icon: Star,
          onclick: () => void mutate(ids, (list) => files.patch(list, { starred: !file.starred })),
        },
        {
          label: file.archived ? 'Unarchive' : 'Archive',
          icon: Archive,
          onclick: () => void mutate(ids, (list) => files.patch(list, { archived: !file.archived })),
        },
        {
          label: 'Move',
          icon: FolderSymlink,
          onclick: () => (moveDialog = { open: true, folder: null }),
        },
        {
          label: 'Move to trash',
          icon: Trash2,
          danger: true,
          onclick: () => void mutate(ids, (list) => files.trash(list)),
        },
      );
    }

    entries.push({ label: 'Details', icon: Info, onclick: () => (detailsFor = file) });
    return entries;
  }

  function folderEntries(folder: Folder): MenuEntry[] {
    return [
      {
        label: 'Rename',
        icon: PenLine,
        onclick: () => (folderDialog = { open: true, folder }),
      },
      {
        label: 'Move',
        icon: FolderSymlink,
        onclick: () => (moveDialog = { open: true, folder }),
      },
      {
        label: 'Delete',
        icon: Trash2,
        danger: true,
        onclick: () => void mutateFolder(() => folders.remove(folder.id)),
      },
    ];
  }

  async function deleteForever(): Promise<void> {
    const ids = confirmDelete.ids;
    confirmDelete = { open: false, ids: [] };
    deleting = true;
    try {
      await mutate(ids, (list) => files.remove(list));
    } finally {
      deleting = false;
    }
  }
</script>

<svelte:window {onkeydown} />

<div class="relative flex h-full min-h-0 flex-col">
  <div class="flex h-11 shrink-0 items-center gap-3 border-b border-border px-4 text-xs">
    {#if selected.size > 0}
      <span class="font-medium">{selected.size} selected</span>
      <span class="text-muted">of {files.items.length}</span>
    {:else}
      <span class="truncate text-muted">{scope}</span>
    {/if}
    <div class="flex-1"></div>
    {#if files.loading}
      <span class="flex items-center gap-2 text-muted">
        <span class="size-3.5 animate-spin rounded-full border-2 border-muted border-t-transparent"></span>
        Loading
      </span>
    {/if}
  </div>

  {#if files.error || folders.error}
    <div class="px-4 pt-3">
      <ErrorBanner
        error={files.error ?? folders.error}
        context="The listing could not be loaded"
        ondismiss={() => {
          files.error = null;
          folders.error = null;
        }}
      />
    </div>
  {/if}

  <div
    class="min-h-0 flex-1 overflow-auto"
    role="presentation"
    onclick={clearSelection}
  >
    {#if empty}
      <EmptyState
        icon={emptyView.icon}
        title={emptyView.title}
        hint={emptyView.hint}
      />
    {:else if nav.current.layout === 'grid'}
      <div class="p-4">
        <FileGrid
          items={sortedFiles}
          folders={sortedFolders}
          {selected}
          onselect={selectFile}
          onactivate={(file) => nav.setFile(file.id)}
          onfilemenu={(event, file) => openMenu(event, fileEntries(file))}
          onfolderopen={(folder) => nav.openFolder(folder.id, folder.name)}
          onfoldermenu={(event, folder) => openMenu(event, folderEntries(folder))}
          onblank={clearSelection}
        />
      </div>
    {:else}
      <FileTable
        items={sortedFiles}
        folders={sortedFolders}
        {selected}
        {sort}
        onsort={toggleSort}
        onselect={selectFile}
        onactivate={(file) => nav.setFile(file.id)}
        onfilemenu={(event, file) => openMenu(event, fileEntries(file))}
        onfolderopen={(folder) => nav.openFolder(folder.id, folder.name)}
        onfoldermenu={(event, folder) => openMenu(event, folderEntries(folder))}
        onblank={clearSelection}
      />
    {/if}
  </div>

  <SelectionBar {...bar} />
</div>

{#if detailsFor}
  <DetailsPanel file={detailsFor} onclose={() => (detailsFor = null)} />
{/if}

<ContextMenu
  open={menu.open}
  x={menu.x}
  y={menu.y}
  entries={menu.entries}
  onclose={() => (menu = { ...menu, open: false })}
/>

<RenameDialog
  open={renameDialog.open}
  file={renameDialog.file}
  onclose={() => (renameDialog = { open: false, file: null })}
  onsave={async (name) => {
    const file = renameDialog.file;
    if (!file) return;
    await files.rename(file.id, name);
    onchanged();
  }}
/>

<FolderDialog
  open={folderDialog.open}
  mode="rename"
  folderId={folderDialog.folder?.id}
  initialName={folderDialog.folder?.name}
  parentId={folderId}
  onclose={() => (folderDialog = { open: false, folder: null })}
  onchanged={folderChanged}
/>

<FolderPickerDialog
  open={moveDialog.open}
  excludeId={moveDialog.folder?.id ?? null}
  title={moveDialog.folder ? 'Move folder' : 'Move to'}
  onclose={() => (moveDialog = { open: false, folder: null })}
  onpick={(parentId: number | null) => {
    const folder = moveDialog.folder;
    moveDialog = { open: false, folder: null };
    if (folder) {
      void mutateFolder(() => folders.move(folder.id, parentId));
      return;
    }
    const ids = selectedIds;
    clearSelection();
    void mutate(ids, (list) => files.move(list, parentId));
  }}
/>

<Dialog
  open={confirmDelete.open}
  title="Delete forever"
  size="sm"
  onclose={() => (confirmDelete = { open: false, ids: [] })}
>
  <p class="text-sm text-muted">
    {confirmDelete.ids.length === 1
      ? 'This file is removed from Google Drive for good.'
      : `${confirmDelete.ids.length} files are removed from Google Drive for good.`}
  </p>

  {#snippet footer()}
    <Button onclick={() => (confirmDelete = { open: false, ids: [] })}>Cancel</Button>
    <Button variant="danger" busy={deleting} onclick={() => void deleteForever()}>Delete forever</Button>
  {/snippet}
</Dialog>