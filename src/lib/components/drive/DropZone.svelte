<script lang="ts">
  // Window-wide drop target. Tauri reports drags on the webview, not on a DOM element, so the
  // overlay here is the feedback layer: `over` while a drag hovers, `drop` with the paths.
  //
  // A dropped folder is handed over as one path — `import::expand` walks it, which is why the
  // backend takes paths and not files.
  import { getCurrentWebview } from '@tauri-apps/api/webview';
  import { FileUp } from '../../icons';
  import type { UnlistenFn } from '@tauri-apps/api/event';

  interface Props {
    ondrop: (paths: string[]) => void;
    /** Disabled while the first-run screen is up, where there is nothing to upload into. */
    enabled?: boolean;
  }

  let { ondrop, enabled = true }: Props = $props();

  let hovering = $state(false);

  $effect(() => {
    let stop: UnlistenFn | undefined;
    let cancelled = false;

    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (!enabled) return;
        const payload = event.payload;
        if (payload.type === 'over') {
          hovering = true;
        } else if (payload.type === 'leave') {
          hovering = false;
        } else if (payload.type === 'drop') {
          hovering = false;
          const paths = payload.paths ?? [];
          if (paths.length > 0) ondrop(paths);
        }
      })
      .then((unlisten) => {
        // The component can unmount before the listener is registered.
        if (cancelled) unlisten();
        else stop = unlisten;
      })
      .catch(() => {
        // No webview (tests, or a build without the plugin): dragging is simply not wired.
      });

    return () => {
      cancelled = true;
      stop?.();
    };
  });
</script>

{#if hovering}
  <div
    class="pointer-events-none absolute inset-0 z-40 flex flex-col items-center justify-center gap-2 border-2 border-dashed border-accent bg-accent/10 backdrop-blur-[1px]"
  >
    <FileUp size={30} strokeWidth={1.6} class="text-accent" />
    <p class="text-sm font-medium">Drop to upload</p>
    <p class="text-xs text-muted">Folders are walked and recreated as 9Drive folders</p>
  </div>
{/if}