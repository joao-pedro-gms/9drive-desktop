![9Drive cover](https://i.ibb.co.com/35BySv1C/image.png)

# 9Drive Desktop

A native desktop app that turns one or more Google Drive accounts into a single virtual
storage dashboard. Connect your accounts once, see their combined quota in one place, upload
files into a dedicated `9drive` Drive folder, and organize everything in virtual folders —
all without a server, a database server, or an account of your own here.

Built with **Tauri v2** (Rust backend) and **Svelte 5**.

## Status

This repository started as a web app (Express + React + MySQL) and has been rewritten as a native
desktop app. The web app has been cut over: it was removed at the `0.1.0` release and stays
readable in the git history and in [zenhosta/9drive](https://github.com/zenhosta/9drive).

- Roadmap: [`docs/rewrite-plan.md`](docs/rewrite-plan.md)
- Progress: [the epic issues](https://github.com/joao-pedro-gms/9drive-desktop/issues?q=is%3Aissue+label%3Aepic)
- Changelog: [`CHANGELOG.md`](CHANGELOG.md)

Known gaps are listed under **Known limitations** below.

## What the desktop app does

- Connect multiple Google accounts into one virtual dashboard.
- Combined and per-account quota tracking.
- Uploads streamed from disk straight to Google Drive — nothing touches a local server.
- Uploads land under a root `9drive` folder in Drive.
- Upload routing policies: most-available, round-robin, or a fixed priority order.
- Virtual folders, stored locally. Reorganizing never moves anything in Drive.
- Search across every folder by file name.
- Preview images, video, audio, PDF, and exported Google Workspace documents.
- Download with range requests, so scrubbing a large video does not download the whole file.
- Manual and incremental sync back from the `9drive` folder, using Drive change tokens.
- Per-file local metadata: star, archive, trash.
- Resume interrupted uploads. Cancel or retry from the upload panel.
- Your Google refresh tokens are stored in the **OS keychain** (Keychain, Credential Manager,
  Secret Service) — never in a config file and never in the local database.

## Requirements

- Rust stable
- Node.js 20+
- A Google Cloud project with the **Google Drive API** enabled
- A Google OAuth client of type **Desktop app**

There is no MySQL, no Docker, and no `.env` file.

## Google Cloud setup

Desktop apps use a loopback redirect, so no client secret is involved. This is the step that
trips people up, so it is spelled out.

1. Open the [API Library](https://console.developers.google.com/apis/library) and enable
   **Google Drive API**.
2. Go to [OAuth consent screen](https://console.developers.google.com/auth/overview).
   Choose **External**, then fill in the app name, support email, and developer email.
3. Add the scope `https://www.googleapis.com/auth/drive`. This is a **restricted** scope.
4. Under **Test users**, add every Google account that will use the app.

> **Why the app ships with a default OAuth client id.**
> Google deletes OAuth clients that have been unused for six months, and any app you build will
> expire yours. So the app ships with a default client id and lets you override it in
> **Settings → Google OAuth client id**. Paste your own if you have a project of your own.

### Publishing beyond your own accounts

The `drive` scope is restricted, so the consent screen must be **verified** before the app can
be published on a public channel or used by accounts outside your test-user list. Until then,
distribution is limited to test users. This is the main constraint on public releases, not a
technical limitation of the app.

## Known limitations

- **The app is not code-signed.** Windows SmartScreen shows an "unknown publisher" warning and
  macOS Gatekeeper refuses to open the app until it is right-clicked → Open. Signing needs a
  certificate you own; the release workflow is ready to take one as a secret.
- **No in-app updates yet.** The updater needs a signing keypair, which is a repository secret
  rather than code.
- **The Google `drive` scope is restricted.** See **Google Cloud setup** above.
- **Uploads cancel between files, not mid-file.** A file already being sent finishes sending.
- **A Workspace document preview is Drive's export**, capped at 10 MB by Drive itself.
- **Virtual folders are local.** They are not folders in Drive, and they do not sync between
  machines.

## Running in development

Prerequisites: Rust stable (`rust-toolchain.toml` pins 1.99.0), Node.js 20+, and the
[system libraries Tauri needs on Linux](https://tauri.app/start/prerequisites/).

```bash
# frontend tooling
npm install

# desktop app (Vite dev server + Rust backend, with hot reload)
cargo tauri dev
```

`cargo tauri dev` starts the frontend dev server, compiles the Rust side, and opens the window.
Logs from both streams appear in the terminal; the app also writes them to the platform log
directory (**Settings → Data** opens the folder on Windows and Linux).

Before opening a pull request:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run check
npm run build
```

## Project structure

```txt
src/                 Svelte 5 frontend
  lib/ipc.ts         typed IPC boundary — every backend call goes through here
  lib/state.svelte.ts rune-based state: accounts, files, folders, upload, settings
  routes/            first run, file browser, quota, settings
src-tauri/           Tauri crate: app state, IPC commands, the 9drive:// preview protocol
crates/
  drive-core/        domain types, errors, SQLite, sync engine, upload routing
  drive-storage/     StorageProvider trait + Google Drive implementation
  drive-oauth/       loopback listener, PKCE, token exchange, OS keychain
docs/rewrite-plan.md the rewrite plan
```

The Rust side is split in three so that core logic can be tested without rebuilding the whole
Tauri bundle, OAuth mechanics stay free of any `tauri` dependency, and the storage abstraction can
grow a second provider later without touching anything else.

## Installing a release

Installers are published as GitHub Releases, one per platform:

| Platform | File |
| --- | --- |
| Windows | `.msi` (silent, for MDM) and `-setup.exe` (NSIS, for a person) |
| macOS (Apple silicon and Intel) | `.dmg` |
| Linux | `.AppImage` and `.deb` |

Releases are published as **drafts**: open the
[releases page](https://github.com/joao-pedro-gms/9drive-desktop/releases), review the notes, and
publish when you are happy with them.

Until the app is signed, Windows SmartScreen and macOS Gatekeeper will warn on first launch. That
is a signing problem, not a broken build: see **Known limitations**.

### Making a release

```bash
# the version has to match in package.json, tauri.conf.json and Cargo.toml
npm version 0.1.0
git tag v0.1.0 && git push origin v0.1.0
```

The workflow builds all five targets and attaches them to a draft Release.

If the tag-push run fails with **`Resource not accessible by integration`** on *create-a-release*,
this repository's default `GITHUB_TOKEN` is read-only for that event and the release never gets
created. Two ways around it, neither of which needs a code change:

```bash
# re-run the same matrix against the tag that already exists
gh workflow run release.yml --ref main -f tag=v0.1.0
```

or set **Settings → Actions → General → Workflow permissions** to *Read and write permissions*, so
the explicit `permissions: contents: write` in the workflow is honoured on tag pushes too.

## License

Apache License 2.0. See [LICENSE](LICENSE).

Copyright 2026 Joao Pedro Gomes for the desktop rewrite. The original 9Drive web app is
Copyright 2026 Zenhosta — see [NOTICE](NOTICE) and [zenhosta/9drive](https://github.com/zenhosta/9drive).