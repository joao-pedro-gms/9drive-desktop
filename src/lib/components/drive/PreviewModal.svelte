<script lang="ts">
  // Preview modal.
  //
  // Two ways to get bytes, and the choice is not cosmetic:
  //  · media and PDF stream over `9drive://file/<driveFileId>`, so a 2 GB video never crosses
  //    IPC — the webview asks for byte ranges and the Rust side answers them from Drive.
  //  · a Workspace document has no bytes of its own, so it is exported by the provider and comes
  //    back as one blob. Exports are capped at 10 MB by Drive, and the provider rejects more.
  import { Download, ExternalLink, Star } from '../../icons';
  import { getFile, previewData, saveFileAs, toggleStar, type FileItem } from '../../ipc';
  import { files } from '../../state.svelte';
  import { fileKind, formatBytes, formatTimestamp } from '../../format';
  import Button from '../ui/Button.svelte';
  import Dialog from '../ui/Dialog.svelte';
  import ErrorBanner from '../ui/ErrorBanner.svelte';
  import IconButton from '../ui/IconButton.svelte';
  import FileIcon from './FileIcon.svelte';

  interface Props {
    fileId: number | null;
    onclose: () => void;
  }

  let { fileId, onclose }: Props = $props();

  let file = $state<FileItem | null>(null);
  let error = $state<Error | null>(null);
  /** Object URL for an exported Workspace document; revoked when the modal closes. */
  let blobUrl = $state<string | null>(null);
  let saving = $state(false);

  const kind = $derived(file ? fileKind(file) : 'file');

  /** A Workspace file is a Google-native type: Drive exports it, it cannot be streamed. */
  const isWorkspace = $derived(Boolean(file?.mime_type?.startsWith('application/vnd.google-apps.')));

  const streamUrl = $derived(file ? `9drive://file/${file.drive_file_id}` : '');

  $effect(() => {
    const id = fileId;
    if (id === null) {
      file = null;
      return;
    }
    let revoked: string | null = null;
    // Switching files mid-export would otherwise resolve the old request after the new one and
    // show the previous document, leaking its blob (up to Drive's 10 MB export cap) as well.
    let cancelled = false;
    void (async () => {
      error = null;
      try {
        const row = await getFile(id);
        if (cancelled) return;
        file = row;
        if (row.mime_type?.startsWith('application/vnd.google-apps.')) {
          const bytes = await previewData(id);
          if (cancelled) return;
          const blob = new Blob([new Uint8Array(bytes)], { type: 'application/pdf' });
          const url = URL.createObjectURL(blob);
          revoked = url;
          blobUrl = url;
        }
      } catch (err) {
        if (!cancelled) error = err instanceof Error ? err : new Error(String(err));
      }
    })();
    return () => {
      cancelled = true;
      // A new file replaces the old URL; leaking one per open would pin the whole export.
      if (revoked) URL.revokeObjectURL(revoked);
      if (revoked === blobUrl) blobUrl = null;
    };
  });

  async function download(): Promise<void> {
    if (!file) return;
    saving = true;
    error = null;
    try {
      await saveFileAs(file.id);
    } catch (err) {
      // A Drive error or an unwritable path has to reach the user, not the console.
      error = err instanceof Error ? err : new Error(String(err));
    } finally {
      saving = false;
    }
  }

  async function star(): Promise<void> {
    if (!file) return;
    await files.toggleStar(file.id);
    file = { ...file, starred: !file.starred };
  }
</script>

<Dialog
  open={fileId !== null}
  title={file?.name ?? 'Preview'}
  size="lg"
  {onclose}
>
  {#if error}
    <ErrorBanner {error} context="Could not open this file" />
  {:else if !file}
    <div class="flex h-64 items-center justify-center">
      <span class="size-6 animate-spin rounded-full border-2 border-muted border-t-transparent"></span>
    </div>
  {:else}
    <div class="grid gap-4 md:grid-cols-[1fr_220px]">
      <div
        class="flex min-h-64 items-center justify-center overflow-hidden rounded-lg border border-border bg-background"
      >
        {#if kind === 'image'}
          <img src={streamUrl} alt={file.name} class="max-h-[60vh] w-auto object-contain" />
        {:else if kind === 'video'}
          <!-- svelte-ignore a11y_media_has_caption -->
          <video controls autoplay class="max-h-[60vh] w-full bg-black">
            <source src={streamUrl} type={file.mime_type ?? 'video/mp4'} />
          </video>
        {:else if kind === 'audio'}
          <div class="flex w-full flex-col items-center gap-4 p-8">
            <div class="flex size-20 items-center justify-center rounded-full bg-accent/10 text-accent">
              <FileIcon {file} size={30} />
            </div>
            <audio controls autoplay class="w-full">
              <source src={streamUrl} type={file.mime_type ?? 'audio/mpeg'} />
            </audio>
          </div>
        {:else if kind === 'pdf' || isWorkspace}
          <iframe
            title={file.name}
            src={blobUrl ?? streamUrl}
            class="h-[60vh] w-full border-0 bg-white"
          ></iframe>
        {:else}
          <div class="flex flex-col items-center gap-3 p-10 text-center">
            <FileIcon {file} size={44} />
            <p class="text-sm">No preview for this file type</p>
            <p class="max-w-xs text-xs text-muted">
              {file.mime_type ?? 'Unknown type'} — download it to open it in its own app.
            </p>
          </div>
        {/if}
      </div>

      <dl class="flex flex-col gap-3 text-xs">
        <div>
          <dt class="text-muted">Name</dt>
          <dd class="mt-0.5 break-words font-medium">{file.name}</dd>
        </div>
        <div>
          <dt class="text-muted">Size</dt>
          <dd class="mt-0.5">{formatBytes(file.size_bytes)}</dd>
        </div>
        <div>
          <dt class="text-muted">Type</dt>
          <dd class="mt-0.5 break-all">
            {isWorkspace ? `${file.mime_type} (exported)` : (file.mime_type ?? 'Unknown')}
          </dd>
        </div>
        <div>
          <dt class="text-muted">Modified</dt>
          <dd class="mt-0.5">{formatTimestamp(file.updated_at)}</dd>
        </div>
        {#if file.md5_checksum}
          <div>
            <dt class="text-muted">MD5</dt>
            <dd class="mt-0.5 break-all font-mono text-[11px] text-muted">{file.md5_checksum}</dd>
          </div>
        {/if}

        <div class="mt-auto flex flex-col gap-2 pt-3">
          <Button variant="primary" size="sm" icon={Download} busy={saving} onclick={download}>
            Download
          </Button>
          <Button size="sm" icon={Star} onclick={star}>
            {file.starred ? 'Remove star' : 'Add star'}
          </Button>
          {#if isWorkspace}
            <a
              class="inline-flex h-8 items-center justify-center gap-2 rounded-lg border border-border text-xs text-muted transition-colors hover:bg-border/50 hover:text-foreground"
              href={`https://drive.google.com/open?id=${encodeURIComponent(file.drive_file_id)}`}
              target="_blank"
              rel="noreferrer"
            >
              <ExternalLink size={14} strokeWidth={2} />
              Open in Drive
            </a>
          {/if}
        </div>
      </dl>
    </div>
  {/if}
</Dialog>