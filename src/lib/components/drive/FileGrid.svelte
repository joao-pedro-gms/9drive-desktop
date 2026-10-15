<script lang="ts">
  // The grid layout. Mirrors `FileTable`'s props on purpose: the two are interchangeable from the
  // screen's point of view, and neither reads `state.svelte`.
  //
  // One column class is picked per breakpoint rather than five explicit ones, so a narrow window
  // degrades to fewer, wider tiles without a second set of markup.
  import { EllipsisVertical, Folder, Star } from '../../icons';
  import { formatBytes } from '../../format';
  import type { FileItem, Folder as FolderRow } from '../../ipc';
  import FileIcon from './FileIcon.svelte';

  interface Props {
    items: FileItem[];
    folders: FolderRow[];
    selected: Set<number>;
    onselect: (event: MouseEvent, file: FileItem) => void;
    onactivate: (file: FileItem) => void;
    onfilemenu: (event: MouseEvent, file: FileItem) => void;
    onfolderopen: (folder: FolderRow) => void;
    onfoldermenu: (event: MouseEvent, folder: FolderRow) => void;
    /** A folder tile is not part of the file selection, so a plain click on one just clears it. */
    onblank: () => void;
  }

  let { items, folders, selected, onselect, onactivate, onfilemenu, onfolderopen, onfoldermenu, onblank }: Props =
    $props();

  const columns = 'grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4 2xl:grid-cols-5';
</script>

<div class={columns}>
  {#each folders as folder (folder.id)}
    <div
      role="button"
      tabindex="0"
      class="group relative flex cursor-default flex-col items-center gap-1.5 rounded-xl border border-border bg-surface px-3 py-4 text-center transition-colors hover:border-accent/50 hover:bg-border/40 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent"
      onclick={(event) => {
        event.stopPropagation();
        onblank();
      }}
      ondblclick={() => onfolderopen(folder)}
      onkeydown={(event) => {
        if (event.key === 'Enter') onfolderopen(folder);
      }}
      oncontextmenu={(event) => onfoldermenu(event, folder)}
    >
      <Folder size={30} strokeWidth={1.6} class="text-amber-400/90" />
      <span class="line-clamp-2 text-[13px] leading-tight">{folder.name}</span>
    </div>
  {/each}

  {#each items as file (file.id)}
    <div
      role="button"
      tabindex="0"
      class="group relative flex cursor-default flex-col items-center gap-1.5 rounded-xl border px-3 py-4 text-center transition-colors focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent {selected.has(
        file.id,
      )
        ? 'border-accent/60 bg-accent/15'
        : 'border-border bg-surface hover:border-accent/50 hover:bg-border/40'}"
      aria-pressed={selected.has(file.id)}
      onclick={(event) => {
        event.stopPropagation();
        onselect(event, file);
      }}
      ondblclick={() => onactivate(file)}
      onkeydown={(event) => {
        if (event.key === 'Enter') onactivate(file);
      }}
      oncontextmenu={(event) => onfilemenu(event, file)}
    >
      {#if file.starred}
        <Star
          size={14}
          strokeWidth={2.4}
          class="absolute left-2 top-2 text-accent"
        />
      {/if}
      <button
        type="button"
        aria-label="File actions"
        class="absolute right-1.5 top-1.5 flex size-7 items-center justify-center rounded-lg bg-surface/80 text-muted opacity-0 transition-opacity hover:bg-border/70 hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100"
        onclick={(event) => {
          event.stopPropagation();
          onfilemenu(event, file);
        }}
      >
        <EllipsisVertical size={14} strokeWidth={2} />
      </button>
      <FileIcon file={file} size={28} />
      <span class="line-clamp-2 text-[13px] leading-tight" title={file.name}>{file.name}</span>
      <span class="text-[11px] text-muted">{formatBytes(file.size_bytes)}</span>
    </div>
  {/each}
</div>