# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

The web application has been rewritten as a native desktop app, and the cutover has happened: the
old `legacy/` tree is gone from this branch and stays readable in the git history and in
[zenhosta/9drive](https://github.com/zenhosta/9drive).

### Added

- **Tauri v2 desktop app** (`src-tauri/`): shared SQLite state, plugins (single-instance,
  window-state, autostart, dialog, opener, log) and a `9drive://` protocol that streams media
  from Drive with `Range` requests, so previewing a large video never pulls it over IPC.
- **Rust domain crates**: `drive-core` (types, errors, migrations, repositories, sync engine,
  upload queue, routing), `drive-storage` (`StorageProvider` trait + Google Drive), `drive-oauth`
  (loopback listener, PKCE, token exchange, OS keychain).
- **Google OAuth**: loopback redirect on a random port with PKCE, refresh tokens in the OS
  keychain, automatic refresh with a 60 s margin, and a disconnect that revokes with Google.
- **Sync**: per-account `startPageToken`, creating/updating/mark-deleted, with the Drive `9drive`
  folder as the source of truth and the local SQLite rows as a mirror.
- **Uploads**: resumable, streamed from disk in 256 KiB chunks, routed by policy (most-available,
  round-robin, or a fixed priority order), with a cancel/retry queue and progress events.
- **Virtual folders**, local-only, with create, rename, move, delete and recent folders.
- **Svelte 5 frontend**: app shell, file browser (list and grid, sorting, multi-select, context
  menus), upload tray, drag-and-drop onto the window, preview modal (image, video, audio, PDF and
  exported Workspace documents), quota page and settings page.
- **Packaging**: app icons, bundle metadata and a release workflow that builds `.msi`, `.dmg`,
  `.AppImage` and `.deb` from a tag.

### Removed (relative to the web app, cut over with `legacy/`)

These features have no place in a single-user local desktop app and will not come back:

- the whole `legacy/` web app: Express + React + MySQL, Docker Compose, PM2/`update.sh`
- email/password registration and login, JWT sessions, Argon2 hashing
- reCAPTCHA
- API keys and the external `POST /api/v1/uploads` endpoint
- public share links and workspace invites
- audit log page
- encrypted OAuth client-config storage (the OS keychain replaces it)

### Known limitations

- No code signing, so SmartScreen and Gatekeeper warn on first launch.
- No in-app updater: it needs a signing keypair stored as a repository secret.
- Pause and cancel take effect between files, not mid-file.
- Virtual folders are local and do not sync between machines.

[Unreleased]: https://github.com/joao-pedro-gms/9drive-desktop/compare/main...HEAD