<script lang="ts">
  // The glyph in a row or a grid tile. Colour is the only thing distinguishing the kinds, so it
  // has to survive both themes: these are mid-tones, not the theme accent.
  import { File, FileText, Folder, Image, Music, Video } from '../../icons';
  import { fileKind, type FileKind } from '../../format';
  import type { FileItem } from '../../ipc';

  interface Props {
    file: Pick<FileItem, 'name' | 'mime_type'>;
    /** Folder rows reuse this component; a folder is not a file, so it is an explicit prop. */
    folder?: boolean;
    size?: number;
  }

  let { file, folder = false, size = 18 }: Props = $props();

  const kind: FileKind = $derived(folder ? 'file' : fileKind(file));

  const Glyph = $derived(
    folder ? Folder : kind === 'image' ? Image : kind === 'video' ? Video : kind === 'audio' ? Music : kind === 'pdf' || kind === 'document' ? FileText : File,
  );

  const tone = $derived(
    folder
      ? 'text-amber-400/90'
      : kind === 'image'
        ? 'text-emerald-400/90'
        : kind === 'video'
          ? 'text-violet-400/90'
          : kind === 'audio'
            ? 'text-rose-400/90'
            : kind === 'pdf'
              ? 'text-orange-400/90'
              : kind === 'archive'
                ? 'text-yellow-400/90'
                : 'text-muted',
  );
</script>

<Glyph {size} strokeWidth={1.8} class={tone} />