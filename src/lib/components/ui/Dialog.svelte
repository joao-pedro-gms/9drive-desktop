<script lang="ts">
  // Modal dialog. Escape and a click on the backdrop both close it; Tab stays inside the panel,
  // which is the one accessibility rule a webview dialog actually needs.
  import type { Snippet } from 'svelte';
  import { X } from '../../icons';
  import IconButton from './IconButton.svelte';

  interface Props {
    open: boolean;
    title: string;
    /** `wide` is for the preview modal, which needs the whole window. */
    size?: 'sm' | 'md' | 'lg';
    onclose: () => void;
    children: Snippet;
    footer?: Snippet;
  }

  let { open, title, size = 'md', onclose, children, footer }: Props = $props();

  const widths = { sm: 'max-w-sm', md: 'max-w-lg', lg: 'max-w-5xl' } as const;

  let panel = $state<HTMLDivElement | null>(null);

  function onkeydown(event: KeyboardEvent): void {
    if (event.key === 'Escape') {
      event.stopPropagation();
      onclose();
      return;
    }
    if (event.key !== 'Tab' || !panel) return;
    const focusable = panel.querySelectorAll<HTMLElement>(
      'button:not([disabled]), input:not([disabled]), textarea, select, a[href], [tabindex]:not([tabindex="-1"])',
    );
    if (focusable.length === 0) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }
</script>

<svelte:window {onkeydown} />

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-6 backdrop-blur-[2px]"
    onclick={(event) => {
      if (event.target === event.currentTarget) onclose();
    }}
  >
    <div
      bind:this={panel}
      role="dialog"
      aria-modal="true"
      aria-label={title}
      class="flex max-h-full w-full {widths[size]} flex-col overflow-hidden rounded-xl border border-border bg-surface shadow-2xl"
    >
      <header class="flex items-center justify-between gap-3 border-b border-border px-4 py-3">
        <h2 class="truncate text-sm font-semibold">{title}</h2>
        <IconButton icon={X} label="Close" onclick={onclose} />
      </header>
      <div class="min-h-0 flex-1 overflow-auto p-4">
        {@render children()}
      </div>
      {#if footer}
        <footer class="flex items-center justify-end gap-2 border-t border-border px-4 py-3">
          {@render footer()}
        </footer>
      {/if}
    </div>
  </div>
{/if}