<script lang="ts">
  // One-line error strip. Every state class in `state.svelte.ts` holds an `IpcError`; this is
  // how a page shows it without inventing a toast system.
  import { CircleAlert, X } from '../../icons';
  import type { IpcError } from '../../ipc';

  interface Props {
    error: IpcError | Error | null;
    /** The command that failed, for a report the user can read back. */
    context?: string;
    ondismiss?: () => void;
  }

  let { error, context, ondismiss }: Props = $props();

  const code = $derived(error && 'code' in error ? error.code : undefined);
</script>

{#if error}
  <div class="flex items-start gap-2.5 rounded-lg border border-danger/40 bg-danger/10 px-3 py-2.5 text-xs">
    <CircleAlert size={15} strokeWidth={2} class="mt-0.5 shrink-0 text-danger" />
    <div class="min-w-0 flex-1">
      {#if context}
        <p class="font-medium">{context}</p>
      {/if}
      <p class="break-words text-foreground/80">{error.message}</p>
      {#if code && code !== 'unknown'}
        <p class="mt-0.5 font-mono text-[11px] text-muted">{code}</p>
      {/if}
    </div>
    {#if ondismiss}
      <button
        type="button"
        aria-label="Dismiss"
        class="shrink-0 rounded p-0.5 text-muted transition-colors hover:text-foreground"
        onclick={ondismiss}
      >
        <X size={14} strokeWidth={2} />
      </button>
    {/if}
  </div>
{/if}