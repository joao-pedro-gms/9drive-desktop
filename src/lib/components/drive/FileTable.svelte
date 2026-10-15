<script lang="ts" module>
  import type { FileItem } from '../../ipc';

  // The sort type lives with the table because the column headers are what the user clicks to
  // change it; `FileScreen` owns the state and the comparator.
  export type SortKey = 'name' | 'date' | 'size';
  export type SortDir = 'asc' | 'desc';
  export interface Sort {
    key: SortKey;
    dir: SortDir;
  }

  /**
   * The comparator lives beside the sort type rather than in `FileScreen`, so "sort the listing" is
   * one decision in one file. A file with no size sorts as the smallest, not in the middle.
   */
  export function compareFiles(
    a: Pick<FileItem, 'name' | 'size_bytes' | 'updated_at' | 'trashed_at'>,
    b: Pick<FileItem, 'name' | 'size_bytes' | 'updated_at' | 'trashed_at'>,
    by: Sort,
  ): number {
    const sign = by.dir === 'asc' ? 1 : -1;
    if (by.key === 'name') {
      return sign * a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: 'base' });
    }
    if (by.key === 'size') return sign * ((a.size_bytes ?? -1) - (b.size_bytes ?? -1));
    // In the trash the meaningful date is when the file was deleted, not when Drive last touched it.
    return sign * ((a.trashed_at ?? a.updated_at) - (b.trashed_at ?? b.updated_at));
  }
</script>

<script lang="ts">
  // The list layout. Deliberately dumb: it renders folders above files and reports what happened,
  // but it never reads `state.svelte`. Selection, sorting and every mutation belong to the screen,
  // which is also the only place that knows which view is open.
  //
  // Column widths are declared once and used by both the header and the body rows — a grid here
  // would drift the moment one side gained a `hidden` breakpoint.
  import { ArrowUp, EllipsisVertical, Star } from '../../icons';
  import { formatAge, formatBytes } from '../../format';
  // `FileItem` comes from the module script above, which is the outer scope here.
  import type { Folder } from '../../ipc';
  import FileIcon from './FileIcon.svelte';

  interface Props {
    items: FileItem[];
    folders: Folder[];
    selected: Set<number>;
    sort: Sort;
    onsort: (key: SortKey) => void;
    /** Plain click, Ctrl-click and Shift-click all arrive here with the original event. */
    onselect: (event: MouseEvent, file: FileItem) => void;
    /** Double-click or Enter on a file row: open the preview. */
    onactivate: (file: FileItem) => void;
    onfilemenu: (event: MouseEvent, file: FileItem) => void;
    onfolderopen: (folder: Folder) => void;
    onfoldermenu: (event: MouseEvent, folder: Folder) => void;
    /** A folder row is not part of the file selection, so a plain click on one just clears it. */
    onblank: () => void;
  }

  let {
    items,
    folders,
    selected,
    sort,
    onsort,
    onselect,
    onactivate,
    onfilemenu,
    onfolderopen,
    onfoldermenu,
    onblank,
  }: Props = $props();

  const SIZE = 'w-24 shrink-0 text-right';
  const KIND = 'hidden w-44 shrink-0 md:block';
  const MODIFIED = 'hidden w-36 shrink-0 lg:block';
  const ACTION = 'w-10 shrink-0';
</script>

<!-- One marker for all three headers: `icons.ts` has no down arrow, so descending reuses the up
     one flipped rather than importing a second glyph for the same idea. -->
{#snippet sortMark(key: SortKey)}
  {#if sort.key === key}
    <ArrowUp size={12} strokeWidth={2.4} class={sort.dir === 'desc' ? 'rotate-180' : ''} />
  {/if}
{/snippet}

<div role="table" aria-label="Files" class="min-w-[560px]">
  <div
    role="rowgroup"
    class="sticky top-0 z-10 border-b border-border bg-background/95 backdrop-blur-sm"
  >
    <div
      role="row"
      class="flex items-center gap-3 px-4 py-2 text-[11px] font-medium uppercase tracking-wide text-muted"
    >
      <div role="columnheader" class="min-w-0 flex-1">
        <button
          type="button"
          class="flex items-center gap-1 rounded transition-colors hover:text-foreground"
          onclick={() => onsort('name')}
        >
          Name
          {@render sortMark('name')}
        </button>
      </div>
      <div role="columnheader" class={SIZE}>
        <button
          type="button"
          class="ml-auto flex items-center gap-1 rounded transition-colors hover:text-foreground"
          onclick={() => onsort('size')}
        >
          Size
          {@render sortMark('size')}
        </button>
      </div>
      <div role="columnheader" class={KIND}>Kind</div>
      <div role="columnheader" class={MODIFIED}>
        <button
          type="button"
          class="flex items-center gap-1 rounded transition-colors hover:text-foreground"
          onclick={() => onsort('date')}
        >
          Modified
          {@render sortMark('date')}
        </button>
      </div>
      <div role="columnheader" class={ACTION}><span class="sr-only">Actions</span></div>
    </div>
  </div>

  <div role="rowgroup">
    {#each folders as folder (folder.id)}
      <div
        role="row"
        tabindex="0"
        class="group flex cursor-default items-center gap-3 border-b border-border/50 px-4 py-2 text-sm transition-colors hover:bg-border/40 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent"
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
        <div role="cell" class="flex min-w-0 flex-1 items-center gap-3">
          <FileIcon file={{ name: folder.name, mime_type: null }} folder size={18} />
          <span class="truncate">{folder.name}</span>
        </div>
        <div role="cell" class={`${SIZE} text-muted`}>—</div>
        <div role="cell" class={`${KIND} truncate text-muted`}>Folder</div>
        <div role="cell" class={`${MODIFIED} truncate text-muted`}>{formatAge(folder.updated_at)}</div>
        <div role="cell" class={ACTION}>
          <button
            type="button"
            aria-label="Folder actions"
            class="flex size-8 items-center justify-center rounded-lg text-muted opacity-0 transition-colors hover:bg-border/60 hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100"
            onclick={(event) => {
              event.stopPropagation();
              onfoldermenu(event, folder);
            }}
          >
            <EllipsisVertical size={15} strokeWidth={2} />
          </button>
        </div>
      </div>
    {/each}

    {#each items as file (file.id)}
      <div
        role="row"
        tabindex="0"
        class="group flex cursor-default items-center gap-3 border-b border-border/50 px-4 py-2 text-sm transition-colors hover:bg-border/40 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent {selected.has(
          file.id,
        )
          ? 'bg-accent/15'
          : ''}"
        aria-selected={selected.has(file.id)}
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
        <div role="cell" class="flex min-w-0 flex-1 items-center gap-3">
          <FileIcon file={file} size={18} />
          <span class="truncate">{file.name}</span>
          {#if file.starred}<Star size={13} strokeWidth={2.4} class="shrink-0 text-accent" />{/if}
        </div>
        <div role="cell" class={`${SIZE} tabular-nums text-muted`}>{formatBytes(file.size_bytes)}</div>
        <div role="cell" class={`${KIND} truncate text-muted`}>{file.mime_type ?? 'Unknown'}</div>
        <div role="cell" class={`${MODIFIED} truncate text-muted`}>{formatAge(file.updated_at)}</div>
        <div role="cell" class={ACTION}>
          <button
            type="button"
            aria-label="File actions"
            class="flex size-8 items-center justify-center rounded-lg text-muted opacity-0 transition-colors hover:bg-border/60 hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100"
            onclick={(event) => {
              event.stopPropagation();
              onfilemenu(event, file);
            }}
          >
            <EllipsisVertical size={15} strokeWidth={2} />
          </button>
        </div>
      </div>
    {/each}
  </div>
</div>