// Navigation state, kept in the URL query (AGENTS.md: `?folderId=` and friends).
//
// There is no router dependency. The app has six screens and one query string, so a class that
// reads `location.search`, mirrors it into runes and writes it back with `pushState` is smaller
// than any router and keeps the back button working for free.

export type View = 'files' | 'starred' | 'archived' | 'trash' | 'quota' | 'settings';

export const VIEWS: View[] = ['files', 'starred', 'archived', 'trash', 'quota', 'settings'];

const VIEWS_SET = new Set<string>(VIEWS);

/** The whole navigation state. Anything not here is component-local and not worth a history entry. */
export interface Route {
  /** Which screen the sidebar has open. */
  view: View;
  /** Virtual folder the file screens are scoped to; `null` is "all my files". */
  folderId: number | null;
  /** File open in the preview modal; `null` closes it. */
  fileId: number | null;
  /** List or grid, remembered across navigation. */
  layout: 'list' | 'grid';
}

function parseView(raw: string | null): View {
  return raw && VIEWS_SET.has(raw) ? (raw as View) : 'files';
}

/** `?folderId=` has to round-trip `0` and reject `abc` without throwing at the URL boundary. */
function parseId(raw: string | null): number | null {
  if (raw === null || raw === '') return null;
  const value = Number(raw);
  return Number.isInteger(value) && value > 0 ? value : null;
}

function read(): Route {
  const params = new URLSearchParams(window.location.search);
  return {
    view: parseView(params.get('view')),
    folderId: parseId(params.get('folderId')),
    fileId: parseId(params.get('fileId')),
    layout: params.get('layout') === 'grid' ? 'grid' : 'list',
  };
}

class NavState {
  current = $state<Route>(read());
  /** Name of the folder currently open, kept in memory only: the URL carries the id, the tree
   * carries the name, and neither needs a second history entry. */
  folderName = $state<string | null>(null);

  /** Starts `popstate` handling; returns the stop for the caller's `$effect` cleanup. */
  listen(): () => void {
    const onPop = () => {
      this.current = read();
    };
    window.addEventListener('popstate', onPop);
    return () => window.removeEventListener('popstate', onPop);
  }

  /**
   * Writes the route back to the URL.
   *
   * `push: false` replaces the entry instead of adding one, for state the user did not ask for:
   * switching list to grid must not make Back undo the layout and stay on the same screen.
   */
  write(route: Route, push = true): void {
    const current = this.current;
    this.current = route;
    if (current.view === route.view && current.folderId === route.folderId && current.fileId === route.fileId) {
      // Same screen: layout-only change, and no history entry for it.
      this.replaceQuery(route);
      return;
    }
    const query = this.toQuery(route);
    const url = query ? `${window.location.pathname}?${query}` : window.location.pathname;
    if (push) window.history.pushState(null, '', url);
    else window.history.replaceState(null, '', url);
  }

  private replaceQuery(route: Route): void {
    const query = this.toQuery(route);
    const url = query ? `${window.location.pathname}?${query}` : window.location.pathname;
    window.history.replaceState(null, '', url);
  }

  private toQuery(route: Route): string {
    const params = new URLSearchParams();
    if (route.view !== 'files') params.set('view', route.view);
    if (route.folderId !== null) params.set('folderId', String(route.folderId));
    if (route.fileId !== null) params.set('fileId', String(route.fileId));
    if (route.layout === 'grid') params.set('layout', 'grid');
    return params.toString();
  }

  /** Opens a screen, clearing the folder scope and any open preview. */
  go(view: View, folderId: number | null = null): void {
    this.folderName = null;
    this.write({ ...this.current, view, folderId, fileId: null });
  }

  /** Navigates into a virtual folder. Folder screens keep their scope. */
  openFolder(folderId: number | null, name?: string): void {
    this.folderName = name ?? null;
    this.write({ ...this.current, view: 'files', folderId, fileId: null });
  }

  /** Back inside the folder tree, without a history entry per level. */
  upFolder(parentId: number | null, name?: string): void {
    this.folderName = name ?? null;
    this.write({ ...this.current, folderId: parentId, fileId: null });
  }

  /** Opens or closes the preview modal without adding a history entry for the close. */
  setFile(fileId: number | null): void {
    this.write({ ...this.current, fileId }, fileId !== null);
  }

  /** Layout is remembered, never navigable: the current screen stays put. */
  setLayout(layout: 'list' | 'grid'): void {
    this.current = { ...this.current, layout };
    this.replaceQuery(this.current);
  }
}

export const nav = new NavState();

/** `accounts`, `starred`, … are what the sidebar renders; each one is one nav screen. */
export function sidebarKey(view: View): string {
  switch (view) {
    case 'files':
      return 'all-files';
    case 'starred':
      return 'starred';
    case 'archived':
      return 'archived';
    case 'trash':
      return 'trash';
    case 'quota':
      return 'quota';
    case 'settings':
      return 'settings';
  }
}