<script lang="ts">
  // The settings screen: one card per concern, each one owning the form state its own key needs.
  //
  // Two fields are seeded from the settings table rather than from their own singleton: the
  // OAuth client id lives in `oauth.client_id` and `settings.googleClientId` only holds the last
  // value this page saved, so a fresh install has to start from the stored key. Seeding is
  // guarded by a `*Seeded` flag so a later reload of the table cannot overwrite what is being
  // typed.
  import { Check, Database, HardDrive, Moon, Sun, TriangleAlert, Trash2 } from '../lib/icons';
  import { accounts, settings } from '../lib/state.svelte';
  import { theme } from '../lib/theme.svelte';
  import { IpcError, openAppDir, type Account } from '../lib/ipc';
  import { formatAge } from '../lib/format';
  import Button from '../lib/components/ui/Button.svelte';
  import Card from '../lib/components/ui/Card.svelte';
  import Dialog from '../lib/components/ui/Dialog.svelte';
  import ErrorBanner from '../lib/components/ui/ErrorBanner.svelte';
  import Input from '../lib/components/ui/Input.svelte';

  type PolicyChoice = 'most_available' | 'round_robin' | 'fixed';

  let clientId = $state('');
  let clientSeeded = $state(false);
  let savingClient = $state(false);
  let clientError = $state<IpcError | null>(null);

  let policyChoice = $state<PolicyChoice>('most_available');
  let fixedAccounts = $state('');
  let policySeeded = $state(false);
  let savingPolicy = $state(false);
  let policyError = $state<IpcError | null>(null);
  let policyFieldError = $state<string | null>(null);

  let openingData = $state(false);
  let dataError = $state<IpcError | null>(null);

  let disconnecting = $state(false);
  let pendingDisconnect = $state<Account | null>(null);

  /** Local ids for the fixed list, so the hint can point at what actually exists. */
  const accountIds = $derived(accounts.items.map((account) => `${account.id} (${account.email})`).join(', '));

  $effect(() => {
    if (clientSeeded) return;
    const stored: string | undefined = settings.values['oauth.client_id'] || settings.googleClientId;
    if (!stored) return;
    clientId = stored;
    clientSeeded = true;
  });

  $effect(() => {
    if (policySeeded) return;
    const raw: string | undefined = settings.values['upload.policy'];
    if (raw === undefined) return;
    if (raw.startsWith('fixed:')) {
      policyChoice = 'fixed';
      fixedAccounts = raw.slice('fixed:'.length);
    } else {
      policyChoice = raw === 'round_robin' ? 'round_robin' : 'most_available';
    }
    policySeeded = true;
  });

  async function saveClientId(): Promise<void> {
    const value = clientId.trim();
    if (!value) {
      clientError = new IpcError('validation', 'Paste the client id of a Desktop-app OAuth client.');
      return;
    }
    savingClient = true;
    settings.error = null;
    try {
      await settings.saveGoogleClientId(value);
      clientError = settings.error;
    } finally {
      savingClient = false;
    }
  }

  /** The stored value is one string; the form splits it into a choice and a fixed id list. */
  function composePolicy(): { value: string; error: string | null } {
    if (policyChoice !== 'fixed') return { value: policyChoice, error: null };
    const tokens = fixedAccounts
      .split(',')
      .map((part) => part.trim())
      .filter(Boolean);
    if (tokens.length === 0) return { value: '', error: 'List at least one account id.' };
    const ids = tokens.map(Number);
    if (ids.some((id) => !Number.isInteger(id) || id <= 0)) {
      return { value: '', error: 'Account ids are numbers separated by commas, e.g. 1,2.' };
    }
    return { value: `fixed:${ids.join(',')}`, error: null };
  }

  async function savePolicy(): Promise<void> {
    const { value, error } = composePolicy();
    policyFieldError = error;
    if (error || !value) return;
    savingPolicy = true;
    settings.error = null;
    try {
      await settings.save({ 'upload.policy': value });
      policyError = settings.error;
    } finally {
      savingPolicy = false;
    }
  }

  async function openDataFolder(): Promise<void> {
    openingData = true;
    try {
      await openAppDir();
      dataError = null;
    } catch (err) {
      dataError = err instanceof IpcError ? err : new IpcError('unknown', String(err));
    } finally {
      openingData = false;
    }
  }

  async function confirmDisconnect(): Promise<void> {
    const account = pendingDisconnect;
    if (!account) return;
    disconnecting = true;
    try {
      await accounts.disconnect(account.id);
    } finally {
      disconnecting = false;
      pendingDisconnect = null;
    }
  }
</script>

<div class="flex h-full min-h-0 flex-col gap-5 overflow-y-auto p-6">
  <Card title="Accounts">
    <div class="space-y-3">
      {#if accounts.error}
        <ErrorBanner
          error={accounts.error}
          context="Account action failed"
          ondismiss={() => (accounts.error = null)}
        />
      {/if}

      {#each accounts.items as account (account.id)}
        <div class="flex items-center gap-3 rounded-lg border border-border/60 px-3 py-2.5">
          <div class="flex size-9 shrink-0 items-center justify-center rounded-full bg-accent/15 text-accent">
            <HardDrive size={17} strokeWidth={1.8} />
          </div>
          <div class="min-w-0 flex-1">
            <p class="truncate text-sm font-medium">{account.name}</p>
            <p class="truncate text-xs text-muted">{account.email}</p>
          </div>
          <div class="hidden shrink-0 text-right text-[11px] text-muted/70 sm:block">
            <p class="capitalize">{account.provider}</p>
            <p>Last used {formatAge(account.last_used_at)}</p>
          </div>
          <Button
            size="sm"
            variant="danger"
            icon={Trash2}
            disabled={accounts.syncing[account.id] !== undefined}
            onclick={() => (pendingDisconnect = account)}
          >
            Disconnect
          </Button>
        </div>
      {/each}
    </div>
  </Card>

  <Card title="Google OAuth client">
    <div class="space-y-3">
      <p class="text-xs text-muted">
        The app ships with an OAuth client id compiled in, so accounts connect with no setup. The
        field below is an override: paste the client id of a <span class="text-foreground"
          >Desktop app</span
        > OAuth client from your own Google Cloud project (Drive API enabled) if you would rather
        connect through your own quota and your own consent screen.
      </p>

      <Input
        bind:value={clientId}
        label="OAuth client id"
        placeholder="000000000000-xxxxxxxxxxxxxxxx.apps.googleusercontent.com"
        hint="Saved as oauth.client_id. Applied to the next connection attempt."
      />

      <p
        class="flex items-start gap-2 rounded-lg border border-border bg-background px-3 py-2.5 text-[11px] text-muted"
      >
        <TriangleAlert size={14} strokeWidth={2} class="mt-0.5 shrink-0 text-danger" />
        <span>
          A Desktop-app client needs no client secret. Never paste one here — the backend rejects
          anything that looks like a secret, and a secret pasted anywhere would still be a secret
          living in a plain settings file.
        </span>
      </p>

      <ErrorBanner error={clientError} context="Could not save the client id" />

      <div class="flex justify-end">
        <Button variant="primary" icon={Check} busy={savingClient} onclick={saveClientId}>
          Save client id
        </Button>
      </div>
    </div>
  </Card>

  <Card title="Uploads">
    <div class="space-y-3">
      <p class="text-xs text-muted">
        Routing picks one account per upload batch — a batch never spans accounts, so all its files
        land in the same Drive folder.
      </p>

      <label class="flex flex-col gap-1.5">
        <span class="text-xs font-medium text-muted">Routing policy</span>
        <select
          bind:value={policyChoice}
          class="h-9 w-full rounded-lg border border-border bg-background px-3 text-sm outline-none focus:border-accent"
        >
          <option value="most_available">Most available space (default)</option>
          <option value="round_robin">Round robin</option>
          <option value="fixed">Fixed account list</option>
        </select>
      </label>

      {#if policyChoice === 'fixed'}
        <Input
          bind:value={fixedAccounts}
          label="Fixed account list"
          placeholder="1,2"
          hint={accountIds ? `Connected accounts: ${accountIds}` : 'Account ids in priority order.'}
          error={policyFieldError ?? undefined}
        />
      {:else}
        <p class="text-[11px] text-muted/70">
          Stored as <span class="font-mono">upload.policy = {policyChoice}</span>.
        </p>
      {/if}

      <ErrorBanner error={policyError} context="Could not save the routing policy" />

      <div class="flex justify-end">
        <Button variant="primary" busy={savingPolicy} onclick={savePolicy}>Save policy</Button>
      </div>
    </div>
  </Card>

  <Card title="Appearance">
    <div class="space-y-3">
      <p class="text-xs text-muted">
        The palette is swapped on the document root; the choice is stored by the theme module and
        restored before the first paint on the next launch.
      </p>
      <div class="flex gap-2">
        <Button
          size="sm"
          variant={theme.current === 'light' ? 'primary' : 'secondary'}
          icon={Sun}
          onclick={() => theme.apply('light')}
        >
          Light
        </Button>
        <Button
          size="sm"
          variant={theme.current === 'dark' ? 'primary' : 'secondary'}
          icon={Moon}
          onclick={() => theme.apply('dark')}
        >
          Dark
        </Button>
      </div>
    </div>
  </Card>

  <Card title="Data">
    <div class="space-y-3">
      <p class="text-xs text-muted">
        The SQLite database and the log files live in the app data folder. Refresh tokens are
        <span class="text-foreground">not</span> in there — they are stored in your operating
        system's keychain.
      </p>

      <ErrorBanner error={dataError} context="Could not open the app data folder" />

      <div class="flex justify-end">
        <Button icon={Database} busy={openingData} onclick={openDataFolder}>Open data folder</Button>
      </div>
    </div>
  </Card>

  <Card title="About">
    <dl class="space-y-2 text-xs">
      <div class="flex justify-between gap-3">
        <dt class="text-muted">Version</dt>
        <dd class="font-medium">{settings.version || '—'}</dd>
      </div>
      <div class="flex items-start justify-between gap-3">
        <dt class="shrink-0 text-muted">Drive access</dt>
        <dd class="text-right">
          Full Drive scope. The app manages the <span class="text-foreground">9drive</span> folder
          itself, so it needs read and write access to the whole account rather than to one folder.
        </dd>
      </div>
      <div class="flex items-start justify-between gap-3">
        <dt class="shrink-0 text-muted">Built from</dt>
        <dd class="text-right">
          A rewrite of the Zenhosta <span class="text-foreground">9Drive</span> project — same idea,
          native desktop instead of a web server.
        </dd>
      </div>
    </dl>
  </Card>
</div>

<Dialog
  open={pendingDisconnect !== null}
  title="Disconnect account"
  size="sm"
  onclose={() => (pendingDisconnect = null)}
>
  <p class="text-sm text-muted">
    Disconnect <span class="text-foreground">{pendingDisconnect?.email ?? pendingDisconnect?.name}</span>?
    The refresh token is revoked with Google, so this account stops syncing and stops receiving
    uploads. Files already in the local database stay, but nothing new from this account will
    appear.
  </p>

  {#snippet footer()}
    <Button onclick={() => (pendingDisconnect = null)}>Cancel</Button>
    <Button variant="danger" busy={disconnecting} onclick={confirmDisconnect}>Disconnect</Button>
  {/snippet}
</Dialog>