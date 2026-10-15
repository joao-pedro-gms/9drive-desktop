<script lang="ts">
  // One level of the virtual folder tree. Recursive: each expanded folder renders another of
  // these, and children are only fetched when the branch is opened â€” a tree walked eagerly would
  // issue one `list_folders` per node on every navigation.
  import { ChevronRight, Folder, FolderOpen } from '../../icons';
  import { listFolders, type Folder as FolderRow } from '../../ipc';
  // Self-import: recursion through `<svelte:self>` is deprecated in runes mode.
  import FolderTree from './FolderTree.svelte';

  interface Props {
    folder: FolderRow;
    /** `null` at the root of the tree. */
    parentId: number | null;
    depth: number;
    currentFolderId: number | null;
    onopen: (folderId: number) => void;
    oncontextmenu?: (event: MouseEvent, folder: FolderRow) => void;
  }

  let { folder, parentId, depth, currentFolderId, onopen, oncontextmenu }: Props = $props();

  let expanded = $state(false);
  let children = $state<FolderRow[]>([]);
  let loading = $state(false);

  const isCurrent = $derived(currentFolderId === folder.id);
  const hasChildren = $derived(children.length > 0 || expanded);
  // A component in a variable renders directly in runes mode; `<svelte:component>` is deprecated.
  const Glyph = $derived(expanded ? FolderOpen : Folder);

  async function toggle(event: MouseEvent): Promise<void> {
    event.stopPropagation();
    expanded = !expanded;
    if (!expanded || children.length > 0) return;
    loading = true;
    try {
      children = await listFolders(folder.id);
    } finally {
      loading = false;
    }
  }
</script>

<li>
  <div
    class="group flex items-center gap-1 rounded-lg py-1 pr-1 transition-colors hover:bg-border/50 {isCurrent
      ? 'bg-border/70'
      : ''}"
    style="padding-left:{depth * 12 + 4}px"
    role="button"
    tabindex="0"
    onclick={() => onopen(folder.id)}
    onkeydown={(event) => {
      if (event.key === 'Enter' || event.key === ' ') {
        event.preventDefault();
        onopen(folder.id);
      }
    }}
    oncontextmenu={(event) => oncontextmenu?.(event, folder)}
  >
    <button
      type="button"
      class="flex size-5 shrink-0 items-center justify-center rounded text-muted transition-transform hover:text-foreground"
      class:rotate-90={expanded}
      style="visibility:{hasChildren || loading ? 'visible' : 'hidden'}"
      aria-label={expanded ? `Collapse ${folder.name}` : `Expand ${folder.name}`}
      onclick={toggle}
    >
      {#if loading}
        <span class="size-3 animate-spin rounded-full border border-current border-t-transparent"></span>
      {:else}
        <ChevronRight size={13} strokeWidth={2.2} />
      {/if}
    </button>
    <Glyph size={15} strokeWidth={1.9} class="shrink-0 text-amber-400/90" />
    <span class="truncate text-[13px]" class:font-medium={isCurrent}>{folder.name}</span>
  </div>

  {#if expanded}
    <ul>
      {#each children as child (child.id)}
        <FolderTree
          folder={child}
          parentId={folder.id}
          depth={depth + 1}
          {currentFolderId}
          {onopen}
          {oncontextmenu}
        />
      {/each}
      {#if children.length === 0 && !loading}
        <li class="py-1 text-[12px] text-muted" style="padding-left:{depth * 12 + 34}px">Empty</li>
      {/if}
    </ul>
  {/if}
</li>
