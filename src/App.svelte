<script lang="ts">
  // The shell. Boot order matters: `app_info` decides whether there is anything to show, the
  // event subscriptions go up before the first listing so no progress event is missed, and the
  // theme is applied from the stored value before anything paints.
  //
  // Navigation lives in `nav`, which mirrors the URL query. Nothing here keeps a second copy of
  // the route, so Back works and a reload lands on the same screen.
  import { EllipsisVertical, FolderPlus, PenLine, Trash2 } from './lib/icons';
  import { accounts, files, folders, settings, upload } from './lib/state.svelte';
  import { nav, type View } from './lib/nav.svelte';
  import { theme } from './lib/theme.svelte';
  import DriveLayout from './lib/components/drive/DriveLayout.svelte';
  import DropZone from './lib/components/drive/DropZone.svelte';
  import FolderDialog from './lib/components/drive/FolderDialog.svelte';
  import FolderPickerDialog from './lib/components/drive/FolderPickerDialog.svelte';
  import PreviewModal from './lib/components/drive/PreviewModal.svelte';
  import UploadPanel from './lib/components/drive/UploadPanel.svelte';
  import ContextMenu, { type MenuEntry } from './lib/components/ui/ContextMenu.svelte';
  import type { Folder } from './lib/ipc';
  import FirstRun from './routes/FirstRun.svelte';
  import FileScreen from './routes/FileScreen.svelte';
  import QuotaPage from './routes/QuotaPage.svelte';
  import SettingsPage from './routes/SettingsPage.svelte';

  const FILE_VIEWS = new Set<View>(['files', 'starred', 'archived', 'trash']);

  let booted = $state(false);
  /** Bumped after a folder create, rename or delete so the sidebar tree reloads. */
  let treeVersion = $state(0);

  let folderDialog = $state<{ open: boolean; mode: 'create' | 'rename'; folderId?: number; name: string }>({
    open: false,
    mode: 'create',
    name: '',
  });
  let moveDialog = $state<{ open: boolean; folder: Folder | null }>({ open: false, folder: null });
  let menu = $state<{ open: boolean; x: number; y: number; entries: MenuEntry[] }>({
    open: false,
    x: 0,
    y: 0,
    entries: [],
  });
  let trayOpen = $state(true);

  const view = $derived(nav.current.view);
  const isFileView = $derived(FILE_VIEWS.has(view));

  const title = $derived.by(() => {
    if (view === 'starred') return 'Starred';
    if (view === 'archived') return 'Archived';
    if (view === 'trash') return 'Trash';
    if (view === 'quota') return 'Storage';
    if (view === 'settings') return 'Settings';
    // The folder name is memory-only — the URL carries the id, not the name — so a reload or a Back
// navigation into a folder leaves it unknown. Say the id rather than invent a name.
return nav.current.folderId === null ? 'My files' : (nav.folderName ?? `Folder #${nav.current.folderId}`);
  });

  const subtitle = $derived(
    view === 'files' && nav.current.folderId !== null ? 'Virtual folder · stored locally' : undefined,
  );

  $effect(() => {
    // `subscribe` registers Tauri listeners and hands back the stop for each one; the cleanup
    // has to call those, not `subscribe` again, or every re-run doubles the handlers.
    let stopAccounts: (() => void) | undefined;
    let stopUpload: (() => void) | undefined;
    let cancelled = false;

    const stopNav = nav.listen();
    void (async () => {
      await Promise.all([settings.refresh(), settings.load(), theme.load()]);
      await accounts.load();
      const [accountsStop, uploadStop] = await Promise.all([accounts.subscribe(), upload.subscribe()]);
      // The effect can be torn down while the listeners are still being registered.
      if (cancelled) {
        accountsStop();
        uploadStop();
        return;
      }
      stopAccounts = accountsStop;
      stopUpload = uploadStop;
      booted = true;
    })();

    return () => {
      cancelled = true;
      stopNav();
      stopAccounts?.();
      stopUpload?.();
    };
  });

  // Reload the listing whenever the screen changes: the folder scope and the filters are two
  // different query shapes, and `files.load` is the only thing that knows how to build them.
  $effect(() => {
    if (!booted || !isFileView) return;
    void files.load({
      folder_id: view === 'files' ? nav.current.folderId : null,
      starred: view === 'starred' ? true : undefined,
      archived: view === 'archived' ? true : undefined,
      trashed: view === 'trash' ? true : undefined,
      loose_only: view === 'files',
    });
  });

  function search(text: string): void {
    if (!isFileView) return;
    void (text ? files.search(text) : files.load({ folder_id: nav.current.folderId, loose_only: view === 'files' }));
  }

  function startUpload(paths: string[]): void {
    trayOpen = true;
    // Only the files screen is scoped to a folder; uploading from Starred lands in the root.
    const folderId = view === 'files' ? nav.current.folderId : null;
    void upload.start(paths, folderId);
  }

  function folderMenu(event: MouseEvent, folder: Folder): void {
    event.preventDefault();
    menu = {
      open: true,
      x: event.clientX,
      y: event.clientY,
      entries: [
        {
          label: 'Rename',
          icon: PenLine,
          onclick: () => {
            folderDialog = { open: true, mode: 'rename', folderId: folder.id, name: folder.name };
          },
        },
        {
          label: 'Move',
          icon: EllipsisVertical,
          onclick: () => {
            moveDialog = { open: true, folder };
          },
        },
        {
          label: 'Delete',
          icon: Trash2,
          danger: true,
          onclick: () => {
            if (nav.current.folderId === folder.id) nav.openFolder(null);
            void foldersRemove(folder.id);
          },
        },
      ],
    };
  }

  async function foldersRemove(folderId: number): Promise<void> {
    await folders.remove(folderId);
    treeVersion += 1;
    if (isFileView) void files.load({ folder_id: nav.current.folderId, loose_only: true });
  }
</script>

{#if !booted}
  <div class="flex h-screen w-screen items-center justify-center bg-background">
    <span class="size-7 animate-spin rounded-full border-2 border-muted border-t-transparent"></span>
  </div>
{:else if !settings.hasAccounts}
  <FirstRun onconnected={() => settings.refresh()} />
{:else}
  <DriveLayout
    {title}
    {subtitle}
    files={isFileView}
    {treeVersion}
    onback={nav.current.folderId !== null && view === 'files'
      ? () => nav.openFolder(null)
      : undefined}
    onsearch={search}
    onnewfolder={() => (folderDialog = { open: true, mode: 'create', name: '' })}
    onfoldermenu={folderMenu}
    onupload={startUpload}
  >
    {#if view === 'quota'}
      <QuotaPage />
    {:else if view === 'settings'}
      <SettingsPage />
    {:else}
      <FileScreen
        {view}
        folderId={view === 'files' ? nav.current.folderId : null}
        onchanged={() => {
          treeVersion += 1;
          void files.load({
            folder_id: view === 'files' ? nav.current.folderId : null,
            loose_only: view === 'files',
          });
        }}
      />
    {/if}

    {#snippet overlays()}
      <DropZone ondrop={startUpload} />
      {#if trayOpen}<UploadPanel onclose={() => (trayOpen = false)} />{/if}
    {/snippet}
  </DriveLayout>
{/if}

<PreviewModal fileId={nav.current.fileId} onclose={() => nav.setFile(null)} />

<ContextMenu
  open={menu.open}
  x={menu.x}
  y={menu.y}
  entries={menu.entries}
  onclose={() => (menu = { ...menu, open: false })}
/>

<FolderDialog
  open={folderDialog.open}
  mode={folderDialog.mode}
  folderId={folderDialog.folderId}
  initialName={folderDialog.name}
  parentId={nav.current.folderId}
  onclose={() => (folderDialog = { ...folderDialog, open: false })}
  onchanged={() => (treeVersion += 1)}
/>

<FolderPickerDialog
  open={moveDialog.open}
  excludeId={moveDialog.folder?.id ?? null}
  title="Move folder"
  onclose={() => (moveDialog = { open: false, folder: null })}
  onpick={(parentId: number | null) => {
    const folder = moveDialog.folder;
    moveDialog = { open: false, folder: null };
    if (!folder) return;
    void (async () => {
      await folders.move(folder.id, parentId);
      treeVersion += 1;
    })();
  }}
/>