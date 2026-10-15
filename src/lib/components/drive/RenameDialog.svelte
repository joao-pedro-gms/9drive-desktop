<script lang="ts">
  // Name prompt for a file. The twin of `FolderDialog`, with the one extra rule a Drive name has:
  // a rename is one path segment, so a separator in it is a bug rather than a nested name.
  import type { FileItem } from '../../ipc';
  import Button from '../ui/Button.svelte';
  import Dialog from '../ui/Dialog.svelte';

  interface Props {
    open: boolean;
    /** `null` while the screen has nothing to rename; the dialog stays closed in that case. */
    file: FileItem | null;
    onclose: () => void;
    /** The dialog does not know about the state layer — the screen performs the rename. */
    onsave: (name: string) => void | Promise<void>;
  }

  let { open, file, onclose, onsave }: Props = $props();

  let name = $state('');
  let error = $state<string | null>(null);
  let busy = $state(false);
  let field = $state<HTMLInputElement | null>(null);

  // Re-seed the field whenever the dialog is opened for a different file.
  $effect(() => {
    if (open && file) {
      name = file.name;
      error = null;
    }
  });

  // Focus by hand rather than with `autofocus`: the field only exists while the dialog is open,
  // and Svelte's `autofocus` is both deprecated and a warning in every check run.
  $effect(() => {
    if (open) field?.focus();
  });

  async function submit(event?: Event): Promise<void> {
    event?.preventDefault();
    const trimmed = name.trim();
    if (!trimmed) {
      error = 'A file needs a name';
      return;
    }
    if (/[\\/]/.test(trimmed)) {
      error = 'A name cannot contain / or \\';
      return;
    }
    busy = true;
    try {
      await onsave(trimmed);
      error = null;
      onclose();
    } finally {
      busy = false;
    }
  }
</script>

<Dialog {open} title="Rename file" size="sm" {onclose}>
  <form id="rename-file-dialog" class="flex flex-col gap-3" onsubmit={submit}>
    <label class="flex flex-col gap-1.5">
      <span class="text-xs font-medium text-muted">Name</span>
      <input
        bind:this={field}
        bind:value={name}
        placeholder="File name"
        class="h-9 w-full rounded-lg border border-border bg-background px-3 text-sm outline-none focus:border-accent"
      />
    </label>
    {#if error}
      <p class="text-xs text-danger">{error}</p>
    {/if}
  </form>

  {#snippet footer()}
    <Button onclick={onclose}>Cancel</Button>
    <Button variant="primary" type="submit" busy={busy} onclick={() => submit()}>Save</Button>
  {/snippet}
</Dialog>