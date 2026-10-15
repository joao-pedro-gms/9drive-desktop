<script lang="ts">
  // The upload tray: one row per task, progress from `upload://progress`, cancel and retry from
  // the two commands. It reads `upload` state directly — no props, because the state is global
  // and there is exactly one tray.
  //
  // Progress is a percentage of bytes sent; the queue reports `sent` and `total`, and a task with
  // no total yet shows a bar in the "starting" state rather than a wrong 0%.
  import { Check, RefreshCw, TriangleAlert, X } from '../../icons';
  import { upload } from '../../state.svelte';
  import { formatBytes } from '../../format';
  import Button from '../ui/Button.svelte';
  import ErrorBanner from '../ui/ErrorBanner.svelte';

  interface Props {
    onclose: () => void;
  }

  let { onclose }: Props = $props();

  let collapsed = $state(false);

  const entries = $derived(
    Object.values(upload.tasks).sort((a, b) => a.task.task_id - b.task.task_id),
  );

  const finished = $derived(entries.filter((entry) => entry.state === 'finished').length);
  const failed = $derived(entries.filter((entry) => entry.state === 'failed').length);
  const running = $derived(
    entries.filter(
      (entry) => entry.state === 'started' || entry.state === 'running' || entry.state === 'paused',
    ).length,
  );

  /** The queue has stopped reporting, so the panel can get out of the way on its own. */
  const done = $derived(entries.length > 0 && running === 0 && collapsed === false);

  function percent(entry: (typeof entries)[number]): number {
    const total = entry.task.size_bytes;
    if (total <= 0) return 0;
    return Math.min(100, (entry.sent / total) * 100);
  }

  const tone = (state: string): string => {
    switch (state) {
      case 'finished':
        return 'bg-emerald-500';
      case 'failed':
        return 'bg-danger';
      case 'cancelled':
        return 'bg-muted/60';
      case 'paused':
        return 'bg-amber-500';
      default:
        return 'bg-accent';
    }
  };
</script>

{#if entries.length > 0}
  <div
    class="absolute bottom-0 right-4 z-30 w-96 overflow-hidden rounded-t-xl border border-b-0 border-border bg-surface shadow-2xl"
  >
    <header class="flex items-center gap-2 border-b border-border px-3 py-2">
      <button
        type="button"
        class="flex flex-1 items-center gap-2 text-left"
        onclick={() => (collapsed = !collapsed)}
      >
        <span class="text-xs font-semibold">
          {#if running > 0}
            Uploading {running} file{running === 1 ? '' : 's'}
          {:else if failed > 0}
            {failed} failed
          {:else}
            {finished} uploaded
          {/if}
        </span>
        <span class="text-[11px] text-muted">{entries.length} total</span>
      </button>
      {#if running === 0}
        <button
          type="button"
          aria-label="Dismiss"
          class="rounded p-0.5 text-muted hover:text-foreground"
          onclick={onclose}
        >
          <X size={14} strokeWidth={2} />
        </button>
      {/if}
    </header>

    {#if !collapsed}
      {#if done && failed > 0}
        <div class="border-b border-border p-2">
          <ErrorBanner error={upload.error} context="Some uploads failed" />
        </div>
      {/if}
      <ul class="max-h-72 overflow-y-auto">
        {#each entries as entry (entry.task.task_id)}
          {@const pct = percent(entry)}
          <li class="border-b border-border/60 px-3 py-2 last:border-b-0">
            <div class="flex items-center gap-2">
              <span class="min-w-0 flex-1 truncate text-xs">{entry.task.name}</span>
              {#if entry.state === 'finished'}
                <Check size={14} strokeWidth={2.2} class="shrink-0 text-emerald-500" />
              {:else if entry.state === 'failed'}
                <TriangleAlert size={14} strokeWidth={2} class="shrink-0 text-danger" />
              {:else}
                <span class="shrink-0 text-[11px] text-muted">
                  {pct < 1 && entry.sent === 0 ? 'starting' : `${Math.round(pct)}%`}
                </span>
              {/if}
            </div>
            <div class="mt-1.5 flex items-center gap-2">
              <div class="h-1 flex-1 overflow-hidden rounded-full bg-border">
                <div class="h-full rounded-full transition-[width] {tone(entry.state)}" style="width:{pct}%"></div>
              </div>
              <span class="shrink-0 text-[10px] text-muted">{formatBytes(entry.task.size_bytes)}</span>
              {#if entry.state === 'failed'}
                <Button size="sm" variant="ghost" icon={RefreshCw} onclick={() => upload.retry(entry.task.task_id)}>
                  Retry
                </Button>
              {:else if entry.state === 'started' || entry.state === 'running' || entry.state === 'paused'}
                <Button size="sm" variant="ghost" onclick={() => upload.cancel(entry.task.task_id)}>Cancel</Button>
              {/if}
            </div>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
{/if}