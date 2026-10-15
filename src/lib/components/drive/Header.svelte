<script lang="ts">
  // Top bar: where you are, what you are looking at, and the four actions that act on the
  // current selection — sync, upload, layout, theme. Search lives here because it is global.
  import { open as openDialog } from '@tauri-apps/plugin-dialog';
  import {
    ArrowLeft,
    LayoutGrid,
    List,
    Moon,
    RefreshCw,
    Search,
    Sun,
    Upload,
    X,
  } from '../../icons';
  import { accounts, upload } from '../../state.svelte';
  import { nav } from '../../nav.svelte';
  import { theme } from '../../theme.svelte';
  import Button from '../ui/Button.svelte';
  import IconButton from '../ui/IconButton.svelte';

  interface Props {
    title: string;
    subtitle?: string;
    /** File screens get the search box and the layout toggle; quota and settings do not. */
    files?: boolean;
    onback?: () => void;
    onsearch?: (text: string) => void;
    onupload: (paths: string[]) => void;
  }

  let { title, subtitle, files = false, onback, onsearch, onupload }: Props = $props();

  let searchText = $state('');
  let picking = $state(false);

  const SEARCHABLE = new Set(['files', 'starred', 'archived', 'trash']);

  $effect(() => {
    // Clear the box when leaving the file screens, so the next screen never shows a stale filter.
    if (!SEARCHABLE.has(nav.current.view)) {
      searchText = '';
      onsearch?.('');
    }
  });

  function typeSearch(text: string): void {
    searchText = text;
    onsearch?.(text);
  }

  async function pick(): Promise<void> {
    if (picking) return;
    picking = true;
    try {
      const picked = await openDialog({ multiple: true, directory: false, title: 'Upload files' });
      if (Array.isArray(picked) && picked.length > 0) onupload(picked);
      else if (typeof picked === 'string') onupload([picked]);
    } finally {
      picking = false;
    }
  }
</script>

<header class="flex h-14 shrink-0 items-center gap-3 border-b border-border bg-surface px-4">
  <div class="flex min-w-0 items-center gap-2">
    {#if onback}
      <IconButton icon={ArrowLeft} label="Back" onclick={onback} />
    {/if}
    <div class="min-w-0">
      <h1 class="truncate text-sm font-semibold">{title}</h1>
      {#if subtitle}
        <p class="truncate text-[11px] text-muted">{subtitle}</p>
      {/if}
    </div>
  </div>

  {#if files}
    <div class="mx-auto flex w-full max-w-md items-center gap-2 rounded-lg border border-border bg-background px-3">
      <Search size={15} strokeWidth={2} class="shrink-0 text-muted" />
      <input
        bind:value={searchText}
        placeholder="Search files"
        class="h-9 w-full bg-transparent text-sm outline-none placeholder:text-muted/70"
        oninput={() => typeSearch(searchText)}
      />
      {#if searchText}
        <button
          type="button"
          class="shrink-0 rounded p-0.5 text-muted hover:text-foreground"
          aria-label="Clear search"
          onclick={() => typeSearch('')}
        >
          <X size={14} strokeWidth={2} />
        </button>
      {/if}
    </div>
  {:else}
    <div class="flex-1"></div>
  {/if}

  <div class="flex shrink-0 items-center gap-1.5">
    <Button
      variant="ghost"
      size="sm"
      icon={RefreshCw}
      busy={accounts.isSyncing}
      title="Sync with Google Drive"
      onclick={() => accounts.syncGoogle()}
    >
      {accounts.isSyncing ? 'Syncing' : 'Sync'}
    </Button>

    {#if files}
      <Button variant="primary" size="sm" icon={Upload} busy={picking} onclick={pick}>Upload</Button>
      <span class="mx-0.5 h-5 w-px bg-border"></span>
      <IconButton
        icon={List}
        label="List view"
        active={nav.current.layout === 'list'}
        onclick={() => nav.setLayout('list')}
      />
      <IconButton
        icon={LayoutGrid}
        label="Grid view"
        active={nav.current.layout === 'grid'}
        onclick={() => nav.setLayout('grid')}
      />
    {/if}

    <IconButton
      icon={theme.current === 'dark' ? Sun : Moon}
      label={theme.current === 'dark' ? 'Light theme' : 'Dark theme'}
      onclick={() => theme.toggle()}
    />

    {#if upload.activeCount > 0}
      <span class="ml-1 rounded-full bg-accent/15 px-2 py-0.5 text-[11px] font-medium text-accent">
        {upload.activeCount} uploading
      </span>
    {/if}
  </div>
</header>