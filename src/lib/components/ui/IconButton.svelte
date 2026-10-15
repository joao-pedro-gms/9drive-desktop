<script lang="ts">
  // Icon-only button. `label` is the accessible name and the tooltip — there is no separate
  // title prop to keep in sync with one.
  import type { IconComponent } from '../../icons';

  interface Props {
    icon: IconComponent;
    label: string;
    active?: boolean;
    disabled?: boolean;
    onclick?: (event: MouseEvent) => void;
  }

  let { icon: Glyph, label, active = false, disabled = false, onclick }: Props = $props();

  const classes = $derived(
    'inline-flex size-9 items-center justify-center rounded-lg transition-colors ' +
      'focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent ' +
      'disabled:pointer-events-none disabled:opacity-45 ' +
      (active ? 'bg-border/70 text-foreground' : 'text-muted hover:bg-border/50 hover:text-foreground'),
  );
</script>

<button type="button" class={classes} title={label} aria-label={label} {disabled} {onclick}>
  <Glyph size={17} strokeWidth={2} />
</button>