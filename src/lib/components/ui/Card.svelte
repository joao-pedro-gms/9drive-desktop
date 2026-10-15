<script lang="ts">
  // Bordered surface for a panel or a page section. The border colour comes from the theme
  // token, so light mode needs no second style.
  import type { Snippet } from 'svelte';

  interface Props {
    title?: string;
    /** Renders `actions` on the title row, right-aligned. */
    actions?: Snippet;
    padded?: boolean;
    children: Snippet;
  }

  let { title, actions, padded = true, children }: Props = $props();
</script>

<section class="rounded-xl border border-border bg-surface">
  {#if title}
    <header class="flex items-center justify-between gap-3 border-b border-border px-4 py-3">
      <h2 class="text-sm font-semibold">{title}</h2>
      {#if actions}
        <div class="flex items-center gap-1">{@render actions()}</div>
      {/if}
    </header>
  {/if}
  <div class={padded ? 'p-4' : ''}>
    {@render children()}
  </div>
</section>