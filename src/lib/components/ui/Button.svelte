<script lang="ts">
  // The one button in the app. `variant` picks the treatment, `size` the box; nothing here
  // knows what a button is for.
  import type { Snippet } from 'svelte';
  import type { IconComponent } from '../../icons';

  interface Props {
    variant?: 'primary' | 'secondary' | 'ghost' | 'danger';
    size?: 'sm' | 'md';
    type?: 'button' | 'submit';
    disabled?: boolean;
    busy?: boolean;
    title?: string;
    icon?: IconComponent;
    onclick?: (event: MouseEvent) => void;
    children: Snippet;
  }

  let {
    variant = 'secondary',
    size = 'md',
    type = 'button',
    disabled = false,
    busy = false,
    title,
    icon: Glyph,
    onclick,
    children,
  }: Props = $props();

  const base =
    'inline-flex items-center justify-center gap-2 rounded-lg font-medium transition-colors ' +
    'focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent ' +
    'disabled:pointer-events-none disabled:opacity-45';

  const sizes = { sm: 'h-8 px-3 text-xs', md: 'h-9 px-4 text-sm' } as const;

  const variants = {
    primary: 'bg-accent text-white hover:bg-accent/85',
    secondary: 'border border-border bg-surface text-foreground hover:bg-border/60',
    ghost: 'text-muted hover:bg-border/50 hover:text-foreground',
    danger: 'bg-danger/90 text-white hover:bg-danger',
  } as const;

  const classes = $derived(`${base} ${sizes[size]} ${variants[variant]}`);
</script>

<button {type} class={classes} {title} disabled={disabled || busy} {onclick}>
  {#if busy}
    <span class="size-3.5 animate-spin rounded-full border-2 border-current border-t-transparent"></span>
  {:else if Glyph}
    <Glyph size={size === 'sm' ? 14 : 16} strokeWidth={2} />
  {/if}
  {@render children()}
</button>