<script lang="ts">
  // Text input with an optional label and hint. `oninput` fires on every keystroke so a search
  // box can filter as the user types.
  import type { Snippet } from 'svelte';

  interface Props {
    value?: string;
    placeholder?: string;
    label?: string;
    hint?: string;
    type?: 'text' | 'password' | 'number';
    autofocus?: boolean;
    disabled?: boolean;
    /** Shown under the field in the danger colour; takes precedence over `hint`. */
    error?: string;
    oninput?: (event: Event & { currentTarget: EventTarget & HTMLInputElement }) => void;
    onkeydown?: (event: KeyboardEvent) => void;
    children?: Snippet;
  }

  let {
    value = $bindable(''),
    placeholder,
    label,
    hint,
    type = 'text',
    autofocus = false,
    disabled = false,
    error,
    oninput,
    onkeydown,
    children,
  }: Props = $props();
</script>

<label class="flex flex-col gap-1.5">
  {#if label}
    <span class="text-xs font-medium text-muted">{label}</span>
  {/if}
  <div class="flex items-center gap-2 rounded-lg border border-border bg-background px-3 focus-within:border-accent">
    {#if children}{@render children()}{/if}
    <!-- Callers pass `autofocus` only for a dialog's one field. -->
    <!-- svelte-ignore a11y_autofocus -->
    <input
      bind:value
      {type}
      {placeholder}
      {disabled}
      {oninput}
      {onkeydown}
      {autofocus}
      class="h-9 w-full bg-transparent text-sm outline-none placeholder:text-muted/70 disabled:opacity-50"
    />
  </div>
  {#if error || hint}
    <span class="text-xs" class:text-danger={Boolean(error)} class:text-muted={!error}>
      {error ?? hint}
    </span>
  {/if}
</label>