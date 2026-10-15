<script lang="ts">
  // Move target picker. The tree is loaded lazily per expanded branch, the same way the
  // sidebar tree is, because a deep folder tree can hold hundreds of nodes.
  //
  // `excludeId` is the subtree being moved: listing it as a destination would let a folder
  // become its own parent, which the backend rejects with a cycle error the user cannot
  // interpret. Hiding it is kinder than reporting it.
  import { ChevronRight, Folder, FolderOpen } from '../../icons';
  import { listFolders, type Folder as FolderRow } from '../../ipc';
  import Button from '../ui/Button.svelte';
  import Dialog from '../ui/Dialog.svelte';

  interface Props {
    open: boolean;
    /** Folder or folder subtree that must not appear as a target. */
    excludeId?: number | null;
    title?: string;
    onclose: () => void;
    onpick: (folderId: number | null) => void;
  }

  let { open, excludeId = null, title = 'Move to', onclose, onpick }: Props = $props();

  let roots = $state<FolderRow[]>([]);
  let expanded = $state<Record<number, FolderRow[]>>({});

  $effect(() => {
    if (!open) return;
    void listFolders(null).then((rows) => {
      roots = rows;
    });
  });

  function allowed(folder: FolderRow): boolean {
    return folder.id !== excludeId;
  }

  async function expand(folder: FolderRow): Promise<void> {
    if (expanded[folder.id]) {
      const next = { ...expanded };
      delete next[folder.id];
      expanded = next;
      return;
    }
    expanded = { ...expanded, [folder.id]: await listFolders(folder.id) };
  }
</script>

{#snippet branch(parentId: number | null, depth: number)}
  {#each (parentId === null ? roots : (expanded[parentId] ?? [])) as folder (folder.id)}
    {#if allowed(folder)}
      {@const Glyph = expanded[folder.id] ? FolderOpen : Folder}
      <li>
        <div
          class="flex items-center gap-1 rounded-lg py-1 transition-colors hover:bg-border/50"
          style="padding-left:{depth * 12}px"
        >
          <button
            type="button"
            class="flex size-5 shrink-0 items-center justify-center rounded text-muted hover:text-foreground"
            aria-label="Toggle"
            onclick={() => expand(folder)}
          >
            <ChevronRight size={13} strokeWidth={2.2} class="rotate-90" />
          </button>
          <button
            type="button"
            class="flex min-w-0 flex-1 items-center gap-2 text-left text-[13px]"
            onclick={() => {
              onpick(folder.id);
              onclose();
            }}
          >
            <Glyph size={15} strokeWidth={1.9} class="shrink-0 text-amber-400/90" />
            <span class="truncate">{folder.name}</span>
          </button>
        </div>
        {@render branch(folder.id, depth + 1)}
      </li>
    {/if}
  {/each}
{/snippet}

<Dialog {open} {title} size="sm" {onclose}>
  <ul class="max-h-72 overflow-y-auto">
    <li>
      <button
        type="button"
        class="flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left text-[13px] transition-colors hover:bg-border/60"
        onclick={() => {
          onpick(null);
          onclose();
        }}
      >
        <Folder size={15} strokeWidth={1.9} class="text-amber-400/90" />
        My files (no folder)
      </button>
    </li>
    {@render branch(null, 1)}
  </ul>

  {#snippet footer()}
    <Button onclick={onclose}>Cancel</Button>
  {/snippet}
</Dialog>