<script lang="ts">
  // Determinate bar. `value` is a percentage that is clamped here, so a caller cannot overflow
  // the track by rounding.
  interface Props {
    /** 0–100. Values outside are clamped. */
    value: number;
    tone?: 'accent' | 'success' | 'danger' | 'muted';
    label?: string;
    /** Renders the percentage inside the bar when it is wide enough to read. */
    showValue?: boolean;
  }

  let { value, tone = 'accent', label, showValue = false }: Props = $props();

  const clamped = $derived(Math.min(100, Math.max(0, Number.isFinite(value) ? value : 0)));

  const tones = {
    accent: 'bg-accent',
    success: 'bg-emerald-500',
    danger: 'bg-danger',
    muted: 'bg-muted/70',
  } as const;
</script>

<div
  class="h-1.5 w-full overflow-hidden rounded-full bg-border"
  role="progressbar"
  aria-valuenow={Math.round(clamped)}
  aria-valuemin="0"
  aria-valuemax="100"
  aria-label={label}
>
  <div class="h-full rounded-full transition-[width] duration-200 {tones[tone]}" style="width:{clamped}%"></div>
</div>