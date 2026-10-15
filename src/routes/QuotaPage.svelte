<script lang="ts">
  // The storage page: one combined card for every connected account, then a card each. It owns
  // no state of its own beyond which confirmation is open — the numbers all live in `accounts`,
  // because the sidebar and the upload router read the same quotas.
  //
  // "Sync all" runs the Drive sync first and only then re-reads the quotas: a quota row is what
  // the backend last saw, so refreshing it before the sync would just re-read the old numbers.
  import { HardDrive, Plus, RefreshCw } from '../lib/icons';
  import { accounts } from '../lib/state.svelte';
  import type { Account, AccountQuota } from '../lib/ipc';
  import { formatAge, formatBytes, formatPercent, percentOf } from '../lib/format';
  import Button from '../lib/components/ui/Button.svelte';
  import Card from '../lib/components/ui/Card.svelte';
  import Dialog from '../lib/components/ui/Dialog.svelte';
  import EmptyState from '../lib/components/ui/EmptyState.svelte';
  import ErrorBanner from '../lib/components/ui/ErrorBanner.svelte';
  import ProgressBar from '../lib/components/ui/ProgressBar.svelte';

  let syncingAll = $state(false);
  let refreshingId = $state<number | null>(null);
  let disconnecting = $state(false);
  let pendingDisconnect = $state<Account | null>(null);

  const combinedPercent = $derived(percentOf(accounts.usageBytes, accounts.limitBytes));
  const combinedFree = $derived(Math.max(0, accounts.limitBytes - accounts.usageBytes));
  const combinedTone = $derived<'accent' | 'danger'>(combinedPercent >= 90 ? 'danger' : 'accent');
  /** Accounts with no quota row yet contribute nothing to the combined figures. */
  const missingQuota = $derived(
    accounts.items.filter((account) => !quotaFor(account.id)).length,
  );

  function quotaFor(accountId: number): AccountQuota | undefined {
    return accounts.quotas[accountId];
  }

  function quotaTone(percent: number): 'accent' | 'danger' {
    return percent >= 90 ? 'danger' : 'accent';
  }

  const syncBusy = $derived(syncingAll || accounts.isSyncing);

  async function syncAll(): Promise<void> {
    syncingAll = true;
    try {
      await accounts.syncGoogle();
      await Promise.all(accounts.items.map((account) => accounts.refreshQuota(account.id)));
    } finally {
      syncingAll = false;
    }
  }

  async function refresh(accountId: number): Promise<void> {
    refreshingId = accountId;
    try {
      await accounts.refreshQuota(accountId);
    } finally {
      refreshingId = null;
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
  {#if accounts.error}
    <ErrorBanner
      error={accounts.error}
      context="Storage action failed"
      ondismiss={() => (accounts.error = null)}
    />
  {/if}

  {#if accounts.loading && accounts.items.length === 0}
    <div class="flex items-center gap-2 text-xs text-muted">
      <span class="size-3.5 animate-spin rounded-full border-2 border-border border-t-transparent"></span>
      Loading accounts…
    </div>
  {:else if accounts.items.length === 0}
    <EmptyState
      icon={HardDrive}
      title="No account connected"
      hint="Connect a Google account and its quota shows up here. Every account you add shares one combined storage total."
    >
      {#snippet action()}
        <Button variant="primary" icon={Plus} busy={accounts.connecting} onclick={() => accounts.connect()}>
          {accounts.connecting ? 'Waiting for Google' : 'Connect another account'}
        </Button>
      {/snippet}
    </EmptyState>
  {:else}
    <Card title="All accounts">
      {#snippet actions()}
        <Button variant="primary" size="sm" icon={RefreshCw} busy={syncBusy} onclick={syncAll}>Sync all</Button>
        <Button size="sm" icon={Plus} busy={accounts.connecting} onclick={() => accounts.connect()}>
          Connect another
        </Button>
      {/snippet}

      <div class="space-y-2.5">
        <div class="flex items-baseline justify-between gap-3">
          <span class="text-2xl font-semibold tabular-nums">{formatBytes(accounts.usageBytes)}</span>
          <span class="text-xs text-muted">of {formatBytes(accounts.limitBytes)}</span>
        </div>

        {#if accounts.limitBytes > 0}
          <ProgressBar value={combinedPercent} tone={combinedTone} label="Combined storage" />
          <div class="flex items-center justify-between text-xs text-muted">
            <span>
              {formatPercent(combinedPercent)} used · {formatBytes(combinedFree)} free
            </span>
            <span>
              {accounts.items.length} account{accounts.items.length === 1 ? '' : 's'}
            </span>
          </div>
        {:else}
          <ProgressBar value={0} tone="muted" label="Combined storage" />
          <p class="text-xs text-muted">
            No quota fetched yet. Use <span class="text-foreground">Sync all</span> to read it from Drive.
          </p>
        {/if}

        {#if missingQuota > 0}
          <p class="text-[11px] text-muted/70">
            {missingQuota} account{missingQuota === 1 ? '' : 's'} not counted here until {missingQuota === 1
              ? 'its'
              : 'their'} quota is fetched.
          </p>
        {/if}
      </div>
    </Card>

    {#each accounts.items as account (account.id)}
      {@const quota = quotaFor(account.id)}
      {@const percent = quota ? percentOf(quota.usage_bytes, quota.limit_bytes) : 0}
      <Card title={account.name}>
        {#snippet actions()}
          <Button
            size="sm"
            variant="ghost"
            icon={RefreshCw}
            busy={refreshingId === account.id}
            disabled={refreshingId !== null && refreshingId !== account.id}
            onclick={() => refresh(account.id)}
          >
            Refresh
          </Button>
          <Button
            size="sm"
            variant="danger"
            disabled={accounts.syncing[account.id] !== undefined}
            onclick={() => (pendingDisconnect = account)}
          >
            Disconnect
          </Button>
        {/snippet}

        <div class="space-y-3">
          <p class="truncate text-xs text-muted">{account.email}</p>

          {#if quota}
            <ProgressBar
              value={percent}
              tone={quotaTone(percent)}
              label={`${account.name} storage`}
            />

            <dl class="grid grid-cols-3 gap-3">
              <div>
                <dt class="text-[11px] text-muted/70">Used</dt>
                <dd class="text-sm font-medium tabular-nums">{formatBytes(quota.usage_bytes)}</dd>
              </div>
              <div>
                <dt class="text-[11px] text-muted/70">Limit</dt>
                <dd class="text-sm font-medium tabular-nums">{formatBytes(quota.limit_bytes)}</dd>
              </div>
              <div>
                <dt class="text-[11px] text-muted/70">Free</dt>
                <dd class="text-sm font-medium tabular-nums">
                  {formatBytes(Math.max(0, quota.limit_bytes - quota.usage_bytes))}
                </dd>
              </div>
            </dl>

            {#if quota.usage_in_drive !== null || quota.usage_in_drive_trash !== null}
              <p class="text-[11px] text-muted">
                In Drive
                {#if quota.usage_in_drive !== null}
                  <span class="text-foreground">{formatBytes(quota.usage_in_drive)}</span>
                {/if}
                {#if quota.usage_in_drive_trash !== null}
                  · <span class="text-foreground">{formatBytes(quota.usage_in_drive_trash)}</span> in trash
                {/if}
              </p>
            {/if}

            <p class="text-[11px] text-muted/70">Synced {formatAge(quota.synced_at)}</p>
          {:else}
            <ProgressBar value={0} tone="muted" label={`${account.name} storage`} />
            <p class="text-xs text-muted">
              Not synced yet — this account's quota has never been fetched from Drive.
            </p>
          {/if}
        </div>
      </Card>
    {/each}
  {/if}
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