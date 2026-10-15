<script lang="ts">
  // First run: no account is connected, so there is nothing to show but the connect button.
  //
  // The two commands are called in order — `begin_google_auth` opens the consent screen and
  // arms the loopback listener, `complete_google_auth` blocks until the browser comes back.
  // The shell only learns whether onboarding is over from `app_info().has_accounts`, so this
  // screen tells the shell rather than deciding for itself.
  import { ExternalLink, HardDrive, TriangleAlert } from '../lib/icons';
  import { accounts } from '../lib/state.svelte';
  import Button from '../lib/components/ui/Button.svelte';
  import ErrorBanner from '../lib/components/ui/ErrorBanner.svelte';

  interface Props {
    onconnected: () => void;
  }

  let { onconnected }: Props = $props();

  const connecting = $derived(accounts.connecting);

  async function connect(): Promise<void> {
    const account = await accounts.connect();
    if (account) onconnected();
  }
</script>

<div class="flex h-screen w-screen items-center justify-center bg-background px-6 text-foreground">
  <div class="w-full max-w-md text-center">
    <div
      class="mx-auto mb-6 flex size-14 items-center justify-center rounded-2xl bg-accent/15 text-accent"
    >
      <HardDrive size={26} strokeWidth={1.7} />
    </div>
    <h1 class="text-xl font-semibold">Welcome to 9Drive</h1>
    <p class="mx-auto mt-2 max-w-sm text-sm text-muted">
      Connect a Google account to start. Files live in a single <span class="text-foreground"
        >9drive</span
      > folder in Drive — everything you upload goes there, and every account you add shares one
      quota.
    </p>

    <div class="mt-7">
      <Button variant="primary" icon={ExternalLink} busy={connecting} onclick={connect}>
        {connecting ? 'Waiting for Google' : 'Connect Google account'}
      </Button>
      {#if connecting}
        <p class="mt-3 text-xs text-muted">
          A browser tab has opened. Approve the request and this window will continue on its own.
        </p>
      {/if}
    </div>

    {#if accounts.error}
      <div class="mt-6 text-left">
        <ErrorBanner error={accounts.error} context="Could not connect the account" />
      </div>
    {/if}

    <p class="mt-8 flex items-start gap-2 rounded-lg border border-border bg-surface px-3 py-2.5 text-left text-[11px] text-muted">
      <TriangleAlert size={14} strokeWidth={2} class="mt-0.5 shrink-0" />
      <span>
        9Drive requests full Drive access because it manages the <span class="text-foreground"
          >9drive</span
        > folder for you. The refresh token is stored in your operating system's keychain, never in
        a file this app can read. You can disconnect an account at any time from Settings, which
        also revokes it with Google.
      </span>
    </p>
  </div>
</div>