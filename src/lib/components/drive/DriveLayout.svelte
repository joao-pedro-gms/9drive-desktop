<script lang="ts">
  // Sidebar + header + content. Everything about the frame lives here so a page only has to
  // render its own body and the upload panel, which hangs off the bottom of the content column.
  import type { Snippet } from 'svelte';
  import Sidebar from './Sidebar.svelte';
  import Header from './Header.svelte';
  import type { Folder } from '../../ipc';

  interface Props {
    title: string;
    subtitle?: string;
    files?: boolean;
    treeVersion: number;
    onback?: () => void;
    onsearch?: (text: string) => void;
    onnewfolder: () => void;
    onfoldermenu: (event: MouseEvent, folder: Folder) => void;
    onupload: (paths: string[]) => void;
    children: Snippet;
    /** Upload panel and other overlays that belong to the window, not to a page. */
    overlays?: Snippet;
  }

  let {
    title,
    subtitle,
    files = false,
    treeVersion,
    onback,
    onsearch,
    onnewfolder,
    onfoldermenu,
    onupload,
    children,
    overlays,
  }: Props = $props();
</script>

<div class="flex h-screen w-screen overflow-hidden bg-background text-foreground">
  <Sidebar {treeVersion} {onnewfolder} {onfoldermenu} />
  <div class="relative flex min-w-0 flex-1 flex-col">
    <Header {title} {subtitle} {files} {onback} {onsearch} {onupload} />
    <main class="relative flex min-h-0 flex-1 flex-col overflow-hidden">
      {@render children()}
      {#if overlays}{@render overlays()}{/if}
    </main>
  </div>
</div>