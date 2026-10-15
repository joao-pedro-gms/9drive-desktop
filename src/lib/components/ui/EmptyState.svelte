<script lang="ts">
  // Empty state for a screen with nothing in it. Distinct from an error: this is the normal
  // appearance of an empty folder, a search with no hits, an empty trash.
  import type { Snippet } from 'svelte';
  import type { IconComponent } from '../../icons';

  interface Props {
    icon: IconComponent;
    title: string;
    hint?: string;
    /** Primary call to action, e.g. "Upload files" in an empty folder. */
    action?: Snippet;
  }

  let { icon: Glyph, title, hint, action }: Props = $props();
</script>

<div class="flex flex-col items-center justify-center gap-3 px-6 py-16 text-center">
  <div class="flex size-12 items-center justify-center rounded-full border border-border bg-surface">
    <Glyph size={22} strokeWidth={1.6} class="text-muted" />
  </div>
  <div class="space-y-1">
    <p class="text-sm font-medium">{title}</p>
    {#if hint}
      <p class="mx-auto max-w-sm text-xs text-muted">{hint}</p>
    {/if}
  </div>
  {#if action}
    <div class="pt-1">{@render action()}</div>
  {/if}
</div>