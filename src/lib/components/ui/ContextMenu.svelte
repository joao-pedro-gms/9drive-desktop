<script lang="ts" module>
  // Shared so a page can build its entries without importing the component's internals.
  export interface MenuEntry {
    label: string;
    icon?: IconComponent;
    /** Greyed out and unclickable; the row stays so the menu does not reflow under the cursor. */
    disabled?: boolean;
    danger?: boolean;
    onclick: () => void;
  }
</script>

<script lang="ts">
  // Right-click menu anchored to a point. Closes on Escape, on a click anywhere, and on a
  // scroll — all three happen to the real cursor, so no bookkeeping of "what was open" is
  // needed: the menu is only rendered while `open` is true.
  import type { Snippet } from 'svelte';
  import type { IconComponent } from '../../icons';

  interface Props {
    open: boolean;
    x: number;
    y: number;
    entries: MenuEntry[];
    onclose: () => void;
  }

  let { open, x, y, entries, onclose }: Props = $props();

  // Flip at the window edge rather than letting the menu run off-screen.
  const position = $derived.by(() => {
    const width = 208;
    const height = entries.length * 30 + 8;
    const left = x + width > window.innerWidth - 8 ? Math.max(8, x - width) : x;
    const top = y + height > window.innerHeight - 8 ? Math.max(8, window.innerHeight - height - 8) : y;
    return `left:${left}px; top:${top}px`;
  });

  function dismiss(): void {
    onclose();
  }

  function onkeydown(event: KeyboardEvent): void {
    if (open && event.key === 'Escape') dismiss();
  }

  function choose(entry: MenuEntry): void {
    if (entry.disabled) return;
    // Close before the action: a rename dialog that opens under the menu is a bug, not a feature.
    dismiss();
    entry.onclick();
  }
</script>

<!-- One window handler for all three ways the menu closes: Escape, a click anywhere, and a
     scroll. A right-click elsewhere is a new menu rather than this one. -->
<svelte:window
  {onkeydown}
  onclick={() => open && dismiss()}
  oncontextmenu={(event) => {
    if (!open) return;
    event.preventDefault();
    dismiss();
  }}
  onscroll={() => open && dismiss()}
/>

{#if open}
  <div
    role="menu"
    class="fixed z-60 min-w-52 rounded-lg border border-border bg-surface py-1 shadow-xl"
    style={position}
  >
    {#each entries as entry (entry.label)}
      <button
        type="button"
        role="menuitem"
        disabled={entry.disabled}
        class="flex w-full items-center gap-2.5 px-3 py-1.5 text-left text-sm transition-colors
          hover:bg-border/60 disabled:pointer-events-none disabled:opacity-40
          {entry.danger ? 'text-danger' : ''}"
        onclick={() => choose(entry)}
      >
        {#if entry.icon}
          <entry.icon size={15} strokeWidth={2} class="shrink-0 opacity-80" />
        {/if}
        <span class="truncate">{entry.label}</span>
      </button>
    {/each}
  </div>
{/if}