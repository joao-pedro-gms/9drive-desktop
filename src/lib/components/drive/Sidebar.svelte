<script lang="ts">
  // The left rail: the fixed destinations, the virtual folder tree, and the storage summary.
  // Navigation goes through `nav`, so the sidebar never holds a copy of the route.
  import {
    Archive,
    Clock,
    FolderPlus,
    HardDrive,
    House,
    Settings,
    Star,
    Trash2,
  } from '../../icons';
  import { listFolders, type Folder as FolderRow } from '../../ipc';
  import { accounts, settings } from '../../state.svelte';
  import { nav, type View } from '../../nav.svelte';
  import { formatBytes, percentOf } from '../../format';
  import FolderTree from './FolderTree.svelte';
  import IconButton from '../ui/IconButton.svelte';
  import ProgressBar from '../ui/ProgressBar.svelte';

  interface Props {
    /** Refreshes the tree root after a create, rename or delete. */
    treeVersion: number;
    onnewfolder: () => void;
    onfoldermenu: (event: MouseEvent, folder: FolderRow) => void;
  }

  let { treeVersion, onnewfolder, onfoldermenu }: Props = $props();

  let roots = $state<FolderRow[]>([]);

  const destinations: { key: View; label: string; icon: typeof House; count?: () => number }[] = [
    { key: 'files', label: 'My files', icon: House },
    { key: 'starred', label: 'Starred', icon: Star },
    { key: 'archived', label: 'Archived', icon: Archive },
    { key: 'trash', label: 'Trash', icon: Trash2 },
  ];

  const usedPercent = $derived(percentOf(accounts.usageBytes, accounts.limitBytes));

  $effect(() => {
    // `treeVersion` is the invalidation key: a mutation bumps it and the root reloads.
    void treeVersion;
    void listFolders(null)
      .then((rows) => {
        roots = rows;
      })
      .catch(() => {
        roots = [];
      });
  });
</script>

<aside class="flex h-full w-60 shrink-0 flex-col border-r border-border bg-surface">
  <div class="flex items-center gap-2.5 px-4 py-4">
    <div class="flex size-8 items-center justify-center rounded-lg bg-accent/15 text-accent">
      <HardDrive size={17} strokeWidth={2} />
    </div>
    <div class="min-w-0">
      <p class="truncate text-sm font-semibold leading-tight">9Drive</p>
      <p class="truncate text-[11px] text-muted">v{settings.version || '—'}</p>
    </div>
  </div>

  <nav class="flex flex-col gap-0.5 px-2">
    {#each destinations as item (item.key)}
      {@const selected = nav.current.view === item.key}
      <button
        type="button"
        class="flex items-center gap-2.5 rounded-lg px-2.5 py-2 text-left text-[13px] transition-colors hover:bg-border/50 {selected
          ? 'bg-border/70 text-foreground'
          : 'text-muted'}"
        onclick={() => nav.go(item.key)}
      >
        <item.icon size={16} strokeWidth={1.9} />
        {item.label}
      </button>
    {/each}
  </nav>

  <div class="mt-4 flex min-h-0 flex-1 flex-col">
    <div class="flex items-center justify-between px-4 pb-1">
      <p class="text-[11px] font-semibold uppercase tracking-wide text-muted">Folders</p>
      <IconButton icon={FolderPlus} label="New folder" onclick={onnewfolder} />
    </div>
    <div class="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
      {#if roots.length === 0}
        <p class="px-2 py-1 text-[12px] text-muted">No folders yet</p>
      {:else}
        <ul>
          {#each roots as folder (folder.id)}
            <FolderTree
              {folder}
              parentId={null}
              depth={0}
              currentFolderId={nav.current.folderId}
              onopen={(folderId) => nav.openFolder(folderId)}
              oncontextmenu={onfoldermenu}
            />
          {/each}
        </ul>
      {/if}
    </div>
  </div>

  <div class="border-t border-border px-4 py-3">
    {#if accounts.limitBytes > 0}
      <div class="mb-2 flex items-baseline justify-between text-[11px]">
        <span class="text-muted">{formatBytes(accounts.usageBytes)} used</span>
        <span class="text-muted">{formatBytes(accounts.limitBytes)}</span>
      </div>
      <ProgressBar value={usedPercent} label="Combined storage used" />
    {/if}
    <div class="mt-3 flex items-center gap-2">
      <button
        type="button"
        class="flex flex-1 items-center gap-2 rounded-lg px-2 py-1.5 text-[13px] transition-colors hover:bg-border/50 hover:text-foreground {nav.current.view ===
        'quota'
          ? 'bg-border/70 text-foreground'
          : 'text-muted'}"
        onclick={() => nav.go('quota')}
      >
        <HardDrive size={15} strokeWidth={1.9} />
        Quota
      </button>
      <button
        type="button"
        class="flex size-8 items-center justify-center rounded-lg transition-colors hover:bg-border/50 hover:text-foreground {nav.current.view ===
        'settings'
          ? 'bg-border/70 text-foreground'
          : 'text-muted'}"
        aria-label="Settings"
        title="Settings"
        onclick={() => nav.go('settings')}
      >
        <Settings size={16} strokeWidth={1.9} />
      </button>
    </div>
  </div>
</aside>