![9Drive cover](https://i.ibb.co.com/35BySv1C/image.png)

# 9Drive Desktop

A native desktop app that turns one or more Google Drive accounts into a single virtual
storage dashboard. Connect your accounts once, see their combined quota in one place, upload
files into a dedicated `9drive` Drive folder, and organize everything in virtual folders —
all without a server, a database server, or an account of your own here.

Built with **Tauri v2** (Rust backend) and **Svelte 5**.

## Status: rewrite in progress

This repository started as a web app (Express + React + MySQL). It is being rewritten as a
native desktop app. The web app now lives under [`legacy/`](legacy/) as read-only reference
and will be removed at cutover.

- Roadmap: [`docs/rewrite-plan.md`](docs/rewrite-plan.md)
- Progress: [the epic issues](https://github.com/joao-pedro-gms/9drive-desktop/issues?q=is%3Aissue+is%3Aopen+label%3Aepic)
- Changelog: [`CHANGELOG.md`](CHANGELOG.md)

The list below describes the target app. Check the epic issues for what actually exists yet.

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

## Running in development

```bash
# frontend tooling
npm install

# desktop app (Vite dev server + Rust backend, with hot reload)
cargo tauri dev
```

`cargo tauri dev` starts the frontend dev server, compiles the Rust side, and opens the window.
Logs from both streams appear in the terminal.

## Project structure

```txt
src/                 Svelte 5 frontend
  lib/ipc.ts         typed IPC boundary — every backend call goes through here
src-tauri/           Tauri crate: app state, IPC commands, OAuth loopback listener
crates/
  9drive-core/       domain types, errors, SQLite, sync engine, upload routing
  9drive-storage/    StorageProvider trait + Google Drive implementation
docs/rewrite-plan.md the rewrite plan
legacy/               legacy web app, removed at cutover
  backend/ frontend/ Express + React + MySQL app
```

The Rust side is split in two so that core logic can be tested without rebuilding the whole
Tauri bundle, and so the storage abstraction can grow a second provider later without touching
anything else.

## License

Apache License 2.0. See [LICENSE](LICENSE).

Copyright 2026 Joao Pedro Gomes for the desktop rewrite. The original 9Drive web app is
Copyright 2026 Zenhosta — see [NOTICE](NOTICE) and [zenhosta/9drive](https://github.com/zenhosta/9drive).