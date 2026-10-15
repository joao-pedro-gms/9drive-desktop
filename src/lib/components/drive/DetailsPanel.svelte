<script lang="ts">
  // Side panel with everything the backend knows about one file. A panel rather than a modal
  // because the point of it is to read metadata about the row you are looking at.
  //
  // The star and archive toggles go through `files.*` so the listing the panel was opened from
  // updates in the same tick — the panel holds the same reactive row, not a copy of it.
  import { Archive, Download, Star, X } from '../../icons';
  import { accounts, files } from '../../state.svelte';
  import { saveFileAs, type FileItem } from '../../ipc';
  import { fileKind, formatBytes, formatTimestamp } from '../../format';
  import Button from '../ui/Button.svelte';
  import ErrorBanner from '../ui/ErrorBanner.svelte';
  import IconButton from '../ui/IconButton.svelte';
  import FileIcon from './FileIcon.svelte';

  interface Props {
    file: FileItem | null;
    onclose: () => void;
  }

  let { file, onclose }: Props = $props();

  let saving = $state(false);
  let error = $state<string | null>(null);

  const account = $derived(
    file ? (accounts.items.find((item) => item.id === file.account_id) ?? null) : null,
  );

  const rows = $derived.by(() => {
    if (!file) return [] as { label: string; value: string }[];
    return [
      { label: 'Size', value: formatBytes(file.size_bytes) },
      { label: 'Kind', value: fileKind(file) },
      { label: 'Type', value: file.mime_type ?? 'Unknown' },
      { label: 'Created', value: formatTimestamp(file.created_at) },
      { label: 'Modified', value: formatTimestamp(file.updated_at) },
      ...(file.trashed_at ? [{ label: 'Deleted', value: formatTimestamp(file.trashed_at) }] : []),
      ...(file.md5_checksum ? [{ label: 'MD5', value: file.md5_checksum }] : []),
      { label: 'Account', value: account ? account.email : `Account #${file.account_id}` },
    ];
  });

  async function download(): Promise<void> {
    if (!file || saving) return;
    saving = true;
    try {
      await saveFileAs(file.id);
      error = null;
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      saving = false;
    }
  }
</script>

{#if file}
  <aside
    aria-label="File details"
    class="absolute inset-y-0 right-0 z-30 flex w-80 shrink-0 flex-col border-l border-border bg-surface"
  >
    <header class="flex items-start gap-3 border-b border-border px-4 py-3">
      <FileIcon file={file} size={24} />
      <div class="min-w-0 flex-1">
        <p class="break-words text-sm font-semibold">{file.name}</p>
      </div>
      <IconButton icon={X} label="Close details" onclick={onclose} />
    </header>

    <div class="min-h-0 flex-1 overflow-auto px-4 py-3">
      {#if error}
        <div class="mb-3">
          <ErrorBanner error={new Error(error)} context="Download failed" />
        </div>
      {/if}

      <dl class="space-y-2.5">
        {#each rows as row (row.label)}
          <div class="flex items-baseline justify-between gap-3 text-xs">
            <dt class="shrink-0 text-muted">{row.label}</dt>
            <dd class="min-w-0 break-all text-right font-mono text-[11px]">{row.value}</dd>
          </div>
        {/each}
      </dl>

      <div class="mt-4 flex items-center gap-1.5 border-t border-border pt-3">
        <IconButton
          icon={Star}
          label={file.starred ? 'Unstar' : 'Star'}
          active={file.starred}
          onclick={() => void files.toggleStar(file!.id)}
        />
        <IconButton
          icon={Archive}
          label={file.archived ? 'Unarchive' : 'Archive'}
          active={file.archived}
          onclick={() => void files.toggleArchive(file!.id)}
        />
        <div class="flex-1"></div>
        <Button size="sm" icon={Download} busy={saving} onclick={() => void download()}>Download</Button>
      </div>
    </div>
  </aside>
{/if}