<script lang="ts">
  // Name prompt for a folder: create and rename are the same dialog with a different title.
  import { folders } from '../../state.svelte';
  import { nav } from '../../nav.svelte';
  import Button from '../ui/Button.svelte';
  import Dialog from '../ui/Dialog.svelte';

  interface Props {
    open: boolean;
    /** `create` ignores the row; `rename` needs the one being renamed. */
    mode: 'create' | 'rename';
    folderId?: number;
    initialName?: string;
    parentId?: number | null;
    onclose: () => void;
    /** Bumped by the shell so the sidebar tree reloads after a change. */
    onchanged: () => void;
  }

  let { open, mode, folderId, initialName = '', parentId = null, onclose, onchanged }: Props = $props();

  let name = $state('');
  let error = $state<string | null>(null);
  let busy = $state(false);

  // Re-seed the field whenever the dialog is opened for a different folder.
  $effect(() => {
    if (open) {
      name = initialName;
      error = null;
    }
  });

  async function submit(event?: Event): Promise<void> {
    event?.preventDefault();
    const trimmed = name.trim();
    if (!trimmed) {
      error = 'A folder needs a name';
      return;
    }
    busy = true;
    try {
      if (mode === 'create') {
        await folders.create(trimmed, parentId ?? nav.current.folderId);
      } else if (folderId !== undefined) {
        await folders.rename(folderId, trimmed);
      }
      if (folders.error) {
        error = folders.error.message;
        return;
      }
      onchanged();
      onclose();
    } finally {
      busy = false;
    }
  }
</script>

<Dialog
  {open}
  title={mode === 'create' ? 'New folder' : 'Rename folder'}
  size="sm"
  {onclose}
>
  <form id="folder-dialog" class="flex flex-col gap-3" onsubmit={submit}>
    <label class="flex flex-col gap-1.5">
      <span class="text-xs font-medium text-muted">Name</span>
      <!-- The name is the only field, so it takes the caret on open. -->
      <!-- svelte-ignore a11y_autofocus -->
      <input
        bind:value={name}
        autofocus
        placeholder="Folder name"
        class="h-9 w-full rounded-lg border border-border bg-background px-3 text-sm outline-none focus:border-accent"
      />
    </label>
    {#if error}
      <p class="text-xs text-danger">{error}</p>
    {/if}
  </form>

  {#snippet footer()}
    <Button onclick={onclose}>Cancel</Button>
    <Button variant="primary" type="submit" busy={busy} onclick={() => submit()}>
      {mode === 'create' ? 'Create' : 'Save'}
    </Button>
  {/snippet}
</Dialog>