# 9drive Desktop — plano de reescrita

> Sessão de planejamento. Nenhum código é tocado aqui.
> Este documento é o plano; a organização do repo, o README e os issues são executados depois, em Build mode.

## 1. Objetivo

Transformar o fork [joao-pedro-gms/9drive-desktop](https://github.com/joao-pedro-gms/9drive-desktop) numa
aplicação desktop nativa: **Tauri v2 + Rust + Svelte 5 + SQLite local**, sem servidor HTTP, sem login,
sem MySQL. O `9drive` web vira histórico (`upstream: zenhosta/9drive`).

## 2. Decisões travadas

| # | Decisão | Motivo |
|---|---|---|
| D1 | **Tauri v2** (não Electron) | binário ~10× menor, reaproveita o Rust como backend real |
| D2 | **Svelte 5** (não React) | escolha do usuário; runes dispensam reatividade manual |
| D3 | **Rust** substitui o Express por inteiro | é o requisito "backend em Rust" |
| D4 | **SQLite via `rusqlite` (bundled)**, não sqlx | sqlx exige `DATABASE_URL` em tempo de compilação ou `.sqlx/` commitado — atrito puro num app sem servidor |
| D5 | **Refresh tokens no keychain do SO** via crate `keyring` | mata `ProviderConfig`, `TOKEN_ENCRYPTION_KEY`, `seed-google-config` e `crypto.ts` |
| D6 | **OAuth loopback + PKCE**, client type *Desktop app* no GCP | recomendado pelo Google para desktop; sem secret não-confidencial |
| D7 | **client_id padrão embutido + override em Settings** | menos atrito agora; escape hatch para distribuição pública |
| D8 | **Cliente Drive REST escrito à mão sobre `reqwest`** | `google-drive3` v7 arrasta hyper 1 + yup-oauth2 + utoipa e é mantido só indiretamente |
| D9 | **S3 adiado**; `StorageProvider` já desenhado para plugar | `aws-sdk-s3` é a dep mais pesada e não bloqueia o core |
| D10 | **Sem `tauri-specta`**; tipos TS escritos à mão em `src/lib/ipc.ts` | ~30 comandos, um arquivo, zero build step |
| D11 | **Workspace Rust de 2 crates** + `src-tauri` | testes de core sem recompilar o Tauri; S3 fora do caminho quente |
| D12 | **Issues por fase**, com épico e checkboxes | rastreio de progresso |
| D13 | `backend/` + `frontend/` vão para `legacy/`, deletados no cutover | referência durante a reescrita; git history + `upstream` preservam o resto |

## 3. O que morre e o que vive

### Morre (não faz sentido num app local single-user)

| Feature atual | Por quê |
|---|---|
| register/login por senha, argon2, JWT, `UserSession` | usuário único; o keychain do SO é o "login" |
| `reCAPTCHA` | não há superfície pública de cadastro |
| `ApiKey` + `POST /api/v1/uploads` + `public-api` | não há servidor HTTP |
| `FileShare`, `PublicFilePage`, `FilePreviewToken` | não há URL pública para compartilhar (Drive já tem link próprio) |
| `WorkspaceInvite`, página de convites | single-user |
| `AuditLog`, `ActivityLogPage` | sem ameaça multi-ator (opcional voltar como log local simples) |
| `ProviderConfig`, `scripts/seed-google-config*`, `crypto.ts` | não há client secret para guardar (D5) |
| `AuthHandoff` (one-time token na URL) | o loopback do OAuth assume esse papel (D6) |
| MySQL + Prisma + `docker-compose.yml` + `setup.ps1/.sh` + nginx | SQLite embarcado (D4) |
| `VITE_API_URL`, `apiFetch`, refresh-retry em `lib/api.ts` | tudo vira IPC |

### Vive

OAuth Google multi-conta · quota (`about.get.storageQuota`) · políticas de roteamento de upload · upload
com resumiable · pastas virtuais (só no banco) · list/grid, busca, renomear, mover, lixeira, starred,
archived, recent · preview (imagem/vídeo/áudio/PDF/export de Workspace doc) · download com Range ·
sync do Drive (pasta `9drive` como fonte da verdade) · tema claro/escuro.

## 4. Arquitetura alvo

```
9drive-desktop/
├── src/                      # Svelte 5 + Vite (frontend)
│   ├── lib/
│   │   ├── ipc.ts            # wrappers tipados dos ~30 commands (D10)
│   │   ├── stores/           # accounts, files, folders, upload, settings
│   │   ├── components/drive/ # portados dos .tsx atuais
│   │   └── components/ui/    # button, card, input, dialog, ...
│   ├── routes/               # all-files, folders, quota, settings, preview
│   ├── App.svelte
│   └── main.ts
├── src-tauri/                # crate Tauri (camada fina)
│   ├── src/
│   │   ├── main.rs  lib.rs  state.rs
│   │   ├── commands/         # accounts, folders, files, upload, sync, settings, app
│   │   └── oauth/            # listener loopback + PKCE (D6)
│   ├── migrations/           # .sql embutidos com include_str!
│   ├── capabilities/  tauri.conf.json  Cargo.toml
├── crates/
│   ├── 9drive-core/          # types, error, config, db (rusqlite), sync, roteamento
│   └── 9drive-storage/       # trait StorageProvider + GoogleDrive (S3 = D9)
├── legacy/                   # backend/ + frontend/ web antigos (sai no cutover)
├── Cargo.toml                # workspace
├── package.json  AGENTS.md  README.md  LICENSE  CHANGELOG.md
└── .github/workflows/ci.yml  release.yml
```

Fluxo: `Svelte UI → invoke() → command Rust → 9drive-core (SQLite + regras) → 9drive-storage → Google Drive API`.
Progresso de upload volta como **evento Tauri** (`upload://progress`), não como polling.

## 5. Modelo de dados (SQLite)

Sem `userId` em lugar nenhum. Tabelas:

- `settings(key TEXT PK, value TEXT)` — override de `google_client_id`, tema, idioma
- `accounts(id, provider, name, email, external_account_id, drive_folder_id, created_at, last_used_at)`
- `account_quota(account_id PK, limit_bytes, usage_bytes, usage_in_drive, usage_in_drive_trash, synced_at)`
- `folders(id, parent_id NULL, name, created_at, updated_at)` — virtual, só no banco
- `files(id, account_id, drive_file_id UNIQUE, folder_id, name, mime_type, size_bytes, md5_checksum, starred, archived, trashed_at, created_at, updated_at)`
- `sync_state(account_id PK, start_page_token, last_sync_at)`
- `upload_routing_policies(id, name, strategy, is_default)`
- `events(id, kind, payload, created_at)` — log local mínimo (opcional, issue própria)
- `_migrations(name PK, applied_at)`

Índices: `files(account_id)`, `files(folder_id)`, `files(name)`, `folders(parent_id)`.

Migrações: `crates/9drive-core/migrations/NNNN_*.sql` embutidos via `include_str!`, aplicadas em ordem
registrada em `_migrations`. Sem tool de migração, sem código gerado.
Acesso: uma `Mutex<Connection>` (rusqlite não é `Sync`) em estado do Tauri, `PRAGMA journal_mode=WAL`.
`shortcut:` conexões separadas por thread só se aparecer pressão de escrita concorrente real.

## 6. `StorageProvider` (crates/9drive-storage)

```rust
#[async_trait]
pub trait StorageProvider: Send + Sync {
    fn kind(&self) -> ProviderKind;
    async fn account_info(&self) -> Result<AccountInfo>;
    async fn quota(&self) -> Result<Quota>;
    async fn ensure_app_folder(&self, name: &str) -> Result<String>;      // cria/retorna "9drive"
    async fn create_folder(&self, parent_id: &str, name: &str) -> Result<String>;
    async fn list_folder(&self, folder_id: &str) -> Result<Vec<RemoteFile>>;
    async fn changes(&self, page_token: Option<&str>) -> Result<ChangePage>; // sync incremental
    async fn upload(&self, req: UploadRequest) -> Result<RemoteFile>;    // resumable
    async fn download(&self, file_id: &str, range: Option<Range>) -> Result<Stream>;
    async fn set_trashed(&self, file_id: &str, trashed: bool) -> Result<()>;
    async fn delete(&self, file_id: &str) -> Result<()>;
}
```

`GoogleDriveProvider` implementa sobre `reqwest` (Drive v3 é REST puro):
`files.list` / `files.create?uploadType=resumable` / `files.get?alt=media` (com `Range`) /
`files.export?mimeType=` (docs do Workspace, máx. 10 MB, checar `capabilities.canDownload`) /
`about.get?fields=storageQuota` / `changes.list?startPageToken`.
Upload resumable: `POST …/upload/drive/v3/files?uploadType=resumable` → lê `Location` → `PUT` com
`Content-Range: bytes a-b/total` → 308 = "continua". Chunk múltiplo de 256 KiB.

## 7. Superfície IPC (~30 commands)

`accounts`: `list_accounts` · `begin_google_auth` · `complete_google_auth` · `disconnect_account` · `sync_account_quota`
`folders`: `list_folders` · `create_folder` · `rename_folder` · `move_folder` · `delete_folder` · `recent_folders`
`files`: `list_files` · `get_file` · `search_files` · `rename_file` · `move_files` · `batch_update` ·
`delete_files` · `trash_files` · `untrash_files` · `toggle_star` · `toggle_archive` · `sync_google`
`upload`: `start_upload_batch` · `cancel_upload` · `retry_upload`
`preview/download`: `preview_data` (bytes p/ blob URL) · `save_file_as`
`settings`: `get_settings` · `set_settings` · `set_google_client_id` · `open_app_dir` · `app_info`

Erros: `IpcError { code: String, message: String }` — `thiserror` no core, serializado na fronteira.
Events: `upload://progress`, `upload://done`, `sync://progress`, `accounts://changed`.

## 8. Fases, épicos e issues

Cada fase = 1 issue épica com checkboxes + issues filhas. Todas as filhas vão no épico via sub-issue.

### F0 · Fundação do repositório — `epic`
1. Mover `backend/` e `frontend/` para `legacy/`
2. Reescrever `.gitignore` (alvo Rust/Node/Tauri) e adicionar `rust-toolchain.toml`
3. Reescrever `AGENTS.md` para a arquitetura nova (Tauri/Rust/Svelte/SQLite) + `CHANGELOG.md`
4. CI: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, `npm run check`, `npm run build`
5. Remover `docker-compose.yml`, `.env.docker.example`, `setup.ps1`, `setup.sh`
6. Manter `LICENSE` (Apache 2.0) + `NOTICE` de atribuição ao upstream Zenhosta

### F1 · Núcleo Rust — `epic`
7. Workspace Cargo + crates `9drive-core` / `9drive-storage` (skeleton)
8. `9drive-core`: tipos de domínio + `thiserror` `AppError` (serializável para IPC)
9. Runner de migrações `include_str!` + `_migrations`
10. Repositório `accounts` + `account_quota` (+ testes com `:memory:`)
11. Repositório `folders` (árvore virtual, mover/ciclicar)
12. Repositório `files` + busca por nome
13. Repositório `settings` + `sync_state`
14. `Cargo.toml` com features `s3` (vazia, placeholder de D9)

### F2 · OAuth Google — `epic`
15. Listener HTTP loopback em porta aleatória (`127.0.0.1`) com página "pode fechar o navegador"
16. Geração PKCE (`code_verifier`/`challenge` S256) + `state`
17. Troca de código por token em `oauth2.googleapis.com/token` (`access_type=offline`)
18. `keyring`: guardar/ler/apagar refresh token por conta
19. Refresh automático com folga de 60 s + reautenticação em `invalid_grant`
20. `about?fields=user` → nome/e-mail da conta; **desconectar revoga** em `oauth2.googleapis.com/revoke`
21. client_id: const padrão compilado + override em Settings, validado com um round-trip
22. Página de Settings → "Contas Google": conectar, listar, sincronizar quota, desconectar

### F3 · Provider Google Drive — `epic`
23. `StorageProvider` trait + `ProviderKind` + erros tipados
24. Cliente HTTP base (base URL, auth, retry com backoff, rate limit 403/429)
25. `ensure_app_folder("9drive")` — idempotente
26. `list_folder` + `create_folder`
27. `changes.list` → `ChangePage` (base do sync incremental)
28. Upload resumable com chunking 256 KiB, retry e `308`
29. Download com `Range` + export de Workspace docs (`canDownload`)
30. `set_trashed` / `delete` definitivo
31. `about.get?fields=storageQuota` → `Quota`

### F4 · Orquestração: upload, sync, roteamento — `epic`
32. Políticas de roteamento (`least-used`, `most-free`, conta fixa) + default
33. Fila de upload: concorrência, pausa/retomar, cancelar, retry, evento de progresso
34. Import de arquivos via diálogo nativo, drag-and-drop da tela, detecção de pastas
35. Motor de sync: `startPageToken` por conta, criar/atualizar/marcar-deletado
36. Metadados locais: star, arquivo, lixeira (`trashed_at`) + filtros
37. Busca por nome em todas as pastas (com `q` da UI → query)

### F5 · Casca Tauri — `epic`
38. `State`: conexão SQLite, config, providers, emitter de eventos
39. Commands de accounts/folders/files + `IpcError`
40. Commands de upload com emissão de `upload://progress`
41. Protocolo custom `9drive://` para preview de mídia grande (stream com Range)
42. Plugins: `single-instance`, `window-state`, `autostart`, `dialog`, `opener`
43. Tela de login/splash enquanto não há conta conectada
44. Log em arquivo (`tracing` + `tauri-plugin-log`)

### F6 · Frontend Svelte — `epic`
45. Shell: `DriveLayout`, sidebar, header com busca, seletor de tema
46. `src/lib/ipc.ts`: tipos e wrappers tipados de todos os commands
47. Stores (`accounts`, `files`, `folders`, `upload`, `settings`) com runes
48. Página "Todos os arquivos": lista + grade, ordenação, seleção múltipla
49. Menu de contexto de arquivo/pasta: renomear, mover, favoritar, arquivar, excluir,properties
50. Criar/renomear/mover/excluir pasta (diálogo + `folderId` na query, como hoje)
51. Painel de upload com progresso por arquivo, cancelar e continuar
52. Arrastar-e-soltar de arquivos/pastas na janela
53. Modal de preview: imagem, vídeo (Plyr → versão Svelte), áudio, PDF, export de Workspace
54. Página de Quota: barra por conta + total combinado + sincronizar
55. Página de Settings: contas, client_id, aparência, dados (abrir pasta do app)
56. Limpar `public`, `index.html` e referências a `VITE_API_URL`

### F7 · Release — `epic`
57. Ícones e metadados do app (Windows/macOS/Linux)
58. `tauri-action` no GitHub Releases + `latest.json` do updater
59. Code signing Windows (EV) + notarização macOS (custo — issue de decisão)
60. `.deb`/AppImage no Linux; `.msi`/`.exe` no Windows; `.dmg` no macOS
61. README final: instalação, OAuth em modo Testing, limitações conhecidas
62. Cortar `legacy/` e marcar o repositório do upstream como origem histórica

## 9. Labels novas

`epic` · `phase:0`…`phase:7` · `area:core|storage|oauth|tauri|ui|infra` · `size:xs|sm|md|lg` · `blocked`.
Manter as existentes (`bug`, `documentation`, `enhancement`, `good first issue`, `help wanted`, …).

## 10. Riscos

1. **Verificação da tela de consentimento.** O escopo `https://www.googleapis.com/auth/drive` é restrito.
   Sem verificação, distribuição pública fica bloqueada em "Testing" + até 100 test users. Mitigação:
   o override de client_id em Settings já previsto (D7) deixa qualquer usuário plugar o próprio
   projeto GCP. **É o risco nº 1 do projeto.**
2. **Limite de refresh tokens do Google** (por cliente/usuário e por usuário). Mitigação: uma
   reautenticação por conta, refresh token guardado no keychain e nunca descartado sem necessidade.
3. **Code signing tem custo** (EV ~US$ 300–700/ano; Apple Developer US$ 99/ano). Issue 59 é de decisão,
   não de código — o app roda sem assinatura, mas o SmartScreen reclama.
4. **`rusqlite` + `Mutex` único** serializa escritas. Aceitável para um app single-user; `shortcut:`
   documentado com o gatilho de migração.
5. **Reescrever 15 páginas e ~30 componentes** do zero em Svelte. Nenhuma lógica é reaproveitada,
   só a estrutura visual e as regras de negócio (mapeadas na §3).

## 11. Ordem de execução

**Nesta sessão (Plan mode):** só este plano.

**Próxima sessão (Build mode), nesta ordem:**
1. F0 — fundação do repo (issue 1–6), um PR só.
2. Criar labels + as 8 issues épicas com checkboxes.
3. Escrever o README novo (desktop) — reescreve os 742 linhas web-oriented para: o que é, features,
   captura, stack, **como rodar em dev** (pré-requisitos Rust/Node, `cargo tauri dev`), **OAuth em modo
   Testing** (o passo que mais trava gente nova), limitações conhecidas, créditos ao upstream, licença.
4. F1 → F7, um PR por fase.

## 12. Critérios de pronto

- `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test` e `npm run check` passam no CI.
- `tauri build` produz instalador para Windows, macOS e Linux a partir de uma tag.
- Fluxo completo: abrir → conectar Google → ver a pasta `9drive` sincronizada → enviar arquivo →
  baixar → navegar em pasta virtual → usar o app offline com o banco local.
- Nenhum segredo no repositório; nenhum refresh token fora do keychain do SO.