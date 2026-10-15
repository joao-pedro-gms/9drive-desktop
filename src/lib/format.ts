// Display helpers. No backend call, no IPC — these turn stored values into what a row shows.
// Everything here is pure so a template can call it freely.

import type { FileItem } from './ipc';

const UNITS = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'];

/** `1536` → `1.5 KB`. Decimal units, because that is what Drive's quota page reports. */
export function formatBytes(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined || Number.isNaN(bytes)) return '—';
  if (bytes < 1000) return `${bytes} B`;
  let value = bytes;
  let unit = 0;
  while (value >= 1000 && unit < UNITS.length - 1) {
    value /= 1000;
    unit += 1;
  }
  // Drive's own labels keep one decimal below 10 and none above it.
  const digits = value < 10 && unit > 0 ? 1 : 0;
  return `${value.toFixed(digits)} ${UNITS[unit]}`;
}

/** Share of a quota, clamped to 0–100 so a rounding error cannot overflow the bar. */
export function percentOf(used: number, limit: number): number {
  if (limit <= 0) return 0;
  return Math.min(100, Math.max(0, (used / limit) * 100));
}

/** Drive reports bytes; the bar reads better with one decimal under 10%. */
export function formatPercent(value: number): string {
  if (value <= 0) return '0%';
  if (value < 10) return `${value.toFixed(1)}%`;
  return `${Math.round(value)}%`;
}

const DATE = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' });
const DATE_ONLY = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium' });

/**
 * Timestamps are Unix **seconds** everywhere (see `commands::now_unix`), so the divisor is
 * fixed rather than guessed. `null` means "never", which the sidebar shows as an em dash.
 */
export function formatTimestamp(seconds: number | null | undefined): string {
  if (!seconds) return '—';
  const date = new Date(seconds * 1000);
  if (Number.isNaN(date.getTime())) return '—';
  return DATE.format(date);
}

export function formatDate(seconds: number | null | undefined): string {
  if (!seconds) return '—';
  const date = new Date(seconds * 1000);
  if (Number.isNaN(date.getTime())) return '—';
  return DATE_ONLY.format(date);
}

/** Rough age for a file list, where the exact time is noise. */
export function formatAge(seconds: number | null | undefined, now = Date.now() / 1000): string {
  if (!seconds) return '—';
  const delta = Math.max(0, Math.floor(now - seconds));
  if (delta < 60) return 'just now';
  if (delta < 3600) {
    const minutes = Math.floor(delta / 60);
    return `${minutes} minute${minutes === 1 ? '' : 's'} ago`;
  }
  if (delta < 86400) {
    const hours = Math.floor(delta / 3600);
    return `${hours} hour${hours === 1 ? '' : 's'} ago`;
  }
  const days = Math.floor(delta / 86400);
  if (days < 30) return `${days} day${days === 1 ? '' : 's'} ago`;
  return formatDate(seconds);
}

export type FileKind = 'image' | 'video' | 'audio' | 'pdf' | 'document' | 'spreadsheet' | 'archive' | 'file';

const BY_MIME: Record<string, FileKind> = {
  'application/pdf': 'pdf',
  'application/zip': 'archive',
  'application/x-tar': 'archive',
  'application/gzip': 'archive',
  'application/x-7z-compressed': 'archive',
  'application/vnd.rar': 'archive',
  'audio/mpeg': 'audio',
  'audio/mp4': 'audio',
  'audio/ogg': 'audio',
  'audio/wav': 'audio',
  'audio/webm': 'audio',
  'video/mp4': 'video',
  'video/webm': 'video',
  'video/quicktime': 'video',
  'video/x-matroska': 'video',
  'video/ogg': 'video',
};

const BY_PREFIX: [string, FileKind][] = [
  ['image/', 'image'],
  ['video/', 'video'],
  ['audio/', 'audio'],
  ['application/vnd.google-apps.', 'document'],
  ['application/vnd.ms-excel', 'spreadsheet'],
  ['application/vnd.openxmlformats-officedocument', 'document'],
  ['application/vnd.oasis.opendocument', 'document'],
  ['text/', 'document'],
];

/**
 * Which visual treatment a file gets. A Workspace document arrives with a
 * `application/vnd.google-apps.*` mime type, which has to be handled before the generic
 * `application/` arm — hence the ordered prefix list.
 */
export function fileKind(file: Pick<FileItem, 'mime_type' | 'name'>): FileKind {
  const mime = (file.mime_type ?? '').toLowerCase();
  const direct = BY_MIME[mime];
  if (direct) return direct;
  for (const [prefix, kind] of BY_PREFIX) {
    if (mime.startsWith(prefix)) return kind;
  }
  // A synced row can arrive with no mime type at all; the extension is the only hint left.
  const extension = file.name.split('.').pop()?.toLowerCase() ?? '';
  if (['jpg', 'jpeg', 'png', 'gif', 'webp', 'svg', 'bmp', 'avif', 'heic'].includes(extension)) return 'image';
  if (['mp4', 'webm', 'mov', 'mkv', 'avi'].includes(extension)) return 'video';
  if (['mp3', 'wav', 'ogg', 'flac', 'm4a'].includes(extension)) return 'audio';
  if (extension === 'pdf') return 'pdf';
  if (['zip', 'tar', 'gz', '7z', 'rar'].includes(extension)) return 'archive';
  return 'file';
}

/** `Report FINAL.pdf` → `final.pdf` for a search field's placeholder example. */
export function baseName(name: string): string {
  const dot = name.lastIndexOf('.');
  return dot > 0 ? name.slice(0, dot) : name;
}

/** Case-insensitive substring match, used for the optimistic filter over an already-listed page. */
export function matchesText(file: FileItem, text: string): boolean {
  const needle = text.trim().toLowerCase();
  if (!needle) return true;
  return file.name.toLowerCase().includes(needle);
}