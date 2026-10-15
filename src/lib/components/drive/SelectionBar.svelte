<script lang="ts">
  // The multi-select action strip. It never touches the state layer: the screen decides what
  // "star" means for the current selection (star vs unstar depends on the rows) and calls back.
  //
  // Trash-only actions are hidden rather than disabled, because a strip that offers "Restore" on a
  // normal listing is a strip with a lie on it.
  import {
    Archive,
    Download,
    FolderSymlink,
    Star,
    Trash2,
    X,
  } from '../../icons';
  import type { IconComponent } from '../../icons';
  import Button from '../ui/Button.svelte';

  interface Props {
    count: number;
    inTrash: boolean;
    allStarred: boolean;
    allArchived: boolean;
    onstar: () => void;
    onarchive: () => void;
    onmove: () => void;
    ontrash: () => void;
    onrestore: () => void;
    ondelete: () => void;
    ondownload: () => void;
    oncancel: () => void;
  }

  let {
    count,
    inTrash,
    allStarred,
    allArchived,
    onstar,
    onarchive,
    onmove,
    ontrash,
    onrestore,
    ondelete,
    ondownload,
    oncancel,
  }: Props = $props();

  type Action = { label: string; icon: IconComponent; onclick: () => void; danger?: boolean };

  const trashActions = $derived<Action[]>([
    { label: 'Restore', icon: FolderSymlink, onclick: onrestore },
    { label: 'Delete forever', icon: Trash2, onclick: ondelete, danger: true },
  ]);

  const normalActions = $derived<Action[]>([
    { label: allStarred ? 'Unstar' : 'Star', icon: Star, onclick: onstar },
    { label: allArchived ? 'Unarchive' : 'Archive', icon: Archive, onclick: onarchive },
    { label: 'Move', icon: FolderSymlink, onclick: onmove },
    { label: 'Move to trash', icon: Trash2, onclick: ontrash },
  ]);

  const actions = $derived(inTrash ? trashActions : normalActions);
</script>

{#if count > 0}
  <div
    class="absolute bottom-4 left-1/2 z-20 flex -translate-x-1/2 items-center gap-2 rounded-xl border border-border bg-surface px-3 py-2 shadow-2xl"
  >
    <span class="px-1 text-xs font-medium text-muted">{count} selected</span>
    <span class="mx-1 h-5 w-px bg-border"></span>
    {#each actions as action (action.label)}
      <Button
        size="sm"
        variant={action.danger ? 'danger' : 'ghost'}
        icon={action.icon}
        onclick={action.onclick}
      >
        {action.label}
      </Button>
    {/each}
    <Button size="sm" variant="ghost" icon={Download} onclick={ondownload}>Download</Button>
    <button
      type="button"
      aria-label="Clear selection"
      class="ml-1 rounded-lg p-1 text-muted transition-colors hover:bg-border/60 hover:text-foreground"
      onclick={oncancel}
    >
      <X size={15} strokeWidth={2} />
    </button>
  </div>
{/if}