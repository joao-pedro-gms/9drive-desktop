# AGENTS.md

## Status

The rewrite is done. `docs/rewrite-plan.md` is the plan it followed; progress was tracked by the
GitHub issues labelled `epic`, and the phase that each PR closed is recorded there.

- **Cut over:** `legacy/backend/` and `legacy/frontend/` — the old Express + React + MySQL web app —
  were removed with the `0.1.0` release. They remain readable in the git history and in the
  upstream, [zenhosta/9drive](https://github.com/zenhosta/9drive).
- **This file describes the code as it is.** Where the Rust and Svelte sources and the CI disagree
  with it, the sources win and this file is what needs fixing.

## Project Overview

9Drive Desktop is a single-user native desktop app that turns one or more Google Drive
accounts into a single virtual storage dashboard. It connects Google accounts through OAuth,
tracks combined quota, uploads files through the app into a dedicated Google Drive `9drive`
folder, organizes files in virtual folders, previews and downloads files, syncs local records
from Google Drive, and routes uploads to the connected account with enough free space.

There is no HTTP server, no user account, and no database server. Everything is local.

## Repository Structure

Target layout:

- `src-tauri/`: Tauri v2 crate. Thin layer: app state, IPC commands, plugins.
- `crates/drive-core/`: domain types, errors, config, SQLite access, migrations, sync engine, upload routing.
- `crates/drive-storage/`: `StorageProvider` trait plus the Google Drive implementation.
- `crates/drive-oauth/`: Google OAuth mechanics. Loopback listener, PKCE, token exchange, keychain.
- `src/`: Svelte 5 frontend. `src/lib/ipc.ts` is the typed IPC boundary.
- `Cargo.toml`: Cargo workspace. `package.json`: frontend tooling only.

Two deviations from `docs/rewrite-plan.md`, both deliberate:

Two deviations from `docs/rewrite-plan.md`, both deliberate and both now real:

- Cargo rejects package names starting with a digit, so the crates drop the plan's `9drive` prefix.
- The plan puts the OAuth loopback listener in `src-tauri/src/oauth/`. That crate did not exist when
  the phases were planned, and a `src-tauri` directory holding no Tauri would be worse than one more
  library crate, so the mechanics live in `drive-oauth` with no `tauri` dependency. The F5 command
  calls into it.

A third deviation came from CI: `src-tauri` links the webview, so the Linux CI and release jobs
install Tauri's system libraries (`libwebkit2gtk-4.1-dev` and friends) before touching Rust. Without
them the build dies in `glib-sys` before it reaches any of our code.

## Requirements

- Rust stable (see `rust-toolchain.toml`)
- Node.js 20+
- Google Cloud project with the Google Drive API enabled
- Google OAuth client of type **Desktop app** (no client secret needed)

## Backend (Rust)

Stack:

- Tauri v2
- tokio, reqwest
- rusqlite (bundled SQLite) with `.sql` migrations embedded via `include_str!`
- thiserror for errors, anyhow only at the binary boundary
- tracing for logs
- keyring for OS keychain storage

Conventions:

- Domain logic lives in `crates/drive-core`; it must not depend on `tauri`.
- All remote storage access goes through the `StorageProvider` trait in `crates/drive-storage`.
  Never call the Google API directly from a command or from `drive-core`.
- Tauri commands stay thin: validate input, call `drive-core`, map errors to `IpcError`.
- SQLite is synchronous. Share one `Mutex<Connection>` in Tauri state; do not open a new
  connection per query without a reason.
- Migrations are numbered `.sql` files in `crates/drive-core/migrations`, applied in order
  and recorded in `_migrations`. Never edit an applied migration; add a new one.
- Store refresh tokens in the OS keychain via `keyring`, never in SQLite and never in a config file.
- Serialize errors across the IPC boundary as `{ code, message }`.

Security rules:

- Never commit `.env` files, OAuth client ids of private projects, or refresh tokens.
- Never log access tokens, refresh tokens, client secrets, or raw share tokens.
- Uploads stream from disk to Drive. Do not buffer whole files in memory.
- Never log file contents.

## Frontend (Svelte 5)

Stack:

- Svelte 5 (runes)
- Vite 8
- TypeScript
- Tailwind CSS 4 via `@tailwindcss/vite`
- `lucide-svelte` for icons

Conventions:

- Use `$state`, `$derived` and `$effect`. Do not add stores for state that belongs to one component.
- Every backend call goes through a typed wrapper in `src/lib/ipc.ts`. No `invoke` in components.
- Keep upload and sync progress in Tauri event listeners (`upload://progress`, `sync://progress`),
  not polling.
- Keep file/folder navigation state in the URL query, e.g. `?folderId=`.
- Preserve the current Tailwind visual style unless a task explicitly asks for a redesign.
- Use the existing `Button`, `Card`, `Input` primitives before adding new UI primitives.

## Verification

Before finishing Rust changes:

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`

Before finishing frontend changes:

- `npm run check`
- `npm run build`

Manual smoke test:

- Open the app with no account connected.
- Connect a Google account from Settings.
- Confirm the `9drive` Drive folder is created and synced.
- Upload a file, watch progress, cancel and retry.
- Create, rename, move and delete a virtual folder.
- Search for an uploaded file by name.
- Preview an image, a PDF and a Workspace document.
- Download a file and verify the bytes.
- Restart the app and confirm state is restored from the local database.

## Agent Rules

- Follow the open epic issues. Do not start work beyond what is asked.
- Prefer small, targeted changes, one PR per phase where practical.
- Preserve existing architecture and naming.
- Do not introduce new dependencies unless necessary, and say why in the PR.
- Do not commit secrets.
- Do not edit build output or generated files. (`src-tauri/gen/` and `dist/` are generated; so is
  everything under `src-tauri/icons/` — change `assets/icon.png` and run `npm run tauri icon`.)
- Do not store anything credential-like outside the OS keychain. `localStorage` may hold the theme
  name and nothing else.
- Do not change the Google OAuth scope without flagging it: `drive` is a restricted scope
  and changing it invalidates existing authorizations.