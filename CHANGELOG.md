# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

The web application is being rewritten as a native desktop app. Until the desktop rewrite
ships, see the `legacy/` directory for the previous Express + React + MySQL application and
the GitHub issues for the rewrite plan.

### Added

- Rewrite plan in `docs/rewrite-plan.md`: Tauri v2 + Rust + Svelte 5 + local SQLite.
- Project phases, epic issues and labels tracking the rewrite.

### Removed (relative to the web app, not yet cut over)

These features have no place in a single-user local desktop app and will not come back:

- email/password registration and login, JWT sessions, Argon2 hashing
- reCAPTCHA
- API keys and the external `POST /api/v1/uploads` endpoint
- public share links and workspace invites
- audit log page
- MySQL, Docker Compose, PM2/`update.sh` self-update
- encrypted OAuth client-config storage (the OS keychain replaces it)

[Unreleased]: https://github.com/joao-pedro-gms/9drive-desktop/compare/main...HEAD