# ks-notes

A self-hosted memo / note-taking server in the spirit of [Memos](https://www.usememos.com/),
built as a **single multithreaded Rust binary**. No Node build, no external database, no
telemetry — one executable plus one SQLite file.

## Features

- **Memos with Markdown** — GFM tables, task lists, strikethrough, autolinks; raw HTML is
  always escaped.
- **Timeline UI** — composer at the top, reverse-chronological feed, infinite scroll,
  inline editing, pin and archive. Server-rendered HTML + [htmx](https://htmx.org);
  works fine with a tiny bit of vanilla JS on top.
- **Activity heatmap & date filter** — a GitHub-style contribution grid over the last
  year, plus a month picker; click any day or pick a month to filter the feed.
- **#tags** — parsed from memo text (code blocks excluded), with a sidebar filter and counts.
- **Full-text search** — SQLite FTS5, live as you type.
- **Visibility & sharing** — `private` / `protected` (any signed-in user) / `public`;
  every memo's permalink `/m/<uid>` doubles as its share link. `/explore` shows the
  shared feed.
- **Attachments** — drag files into the composer; stored as blobs inside the SQLite
  database, images render inline. Non-media uploads are served as downloads so they
  can't run scripts.
- **Multi-user** — first registered account becomes admin; admin can open/close
  registration. Session cookies (httponly, SameSite=Lax), Argon2id password hashing,
  CSRF protection on every mutating form.
- **JSON API** — `/api/v1/*` with `Authorization: Bearer <token>`; tokens are minted in
  Settings, shown once, and stored hashed.

## Build & run

Requires a Rust toolchain (1.96+). SQLite is compiled in (`rusqlite` bundled), so there
is nothing else to install.

```sh
cargo build --release
./target/release/ks-notes --port 5230 --db-path ./data/ks-notes.db
```

Open http://127.0.0.1:5230 — the first account you create is the admin.

### Configuration

| Flag | Env var | Default | Meaning |
|---|---|---|---|
| `--bind` | `KSNOTES_BIND` | `127.0.0.1` | Listen address (use `0.0.0.0` to expose) |
| `--port` | `KSNOTES_PORT` | `5230` | Listen port |
| `--db-path` | `KSNOTES_DB_PATH` | `ks-notes.db` | SQLite file (parent dirs created) |
| `--max-upload-mb` | `KSNOTES_MAX_UPLOAD_MB` | `32` | Request body / upload cap |
| `--secure-cookies` | `KSNOTES_SECURE_COOKIES` | off | Set `Secure` on cookies (enable behind HTTPS) |

Logging uses `RUST_LOG` (e.g. `RUST_LOG=debug`).

**Backup** = copy the `.db` file (plus `-wal`/`-shm` siblings, or run after a clean stop).
Everything — users, memos, attachments — lives in it.

## API quick reference

```sh
TOK=ksn_...   # create in Settings → API tokens

curl -H "Authorization: Bearer $TOK" http://127.0.0.1:5230/api/v1/me
curl -H "Authorization: Bearer $TOK" -H "Content-Type: application/json" \
     -d '{"content":"hello #inbox","visibility":"private"}' \
     http://127.0.0.1:5230/api/v1/memos
curl -H "Authorization: Bearer $TOK" "http://127.0.0.1:5230/api/v1/memos?q=hello&limit=10"
curl -H "Authorization: Bearer $TOK" -X PATCH -H "Content-Type: application/json" \
     -d '{"pinned":true}' http://127.0.0.1:5230/api/v1/memos/<uid>
curl -H "Authorization: Bearer $TOK" -X DELETE http://127.0.0.1:5230/api/v1/memos/<uid>
curl -H "Authorization: Bearer $TOK" http://127.0.0.1:5230/api/v1/tags
```

`GET /api/v1/memos` supports `q`, `tag`, `state=archived`, `limit` (≤100), and keyset
paging via the returned `next_before` cursor.

## Development

```sh
cargo test            # unit + integration tests (in-memory router, temp DBs)
cargo clippy          # lint
cargo run -- --db-path ./data/dev.db
```

Layout: `src/db/` (SQLite, migrations in `db/mod.rs`), `src/routes/` (pages, htmx
fragments, API), `templates/` (Askama, compiled into the binary), `static/` (embedded
via `include_bytes!`).

## License

MIT. Inspired by — but sharing no code with — [usememos/memos](https://github.com/usememos/memos) (also MIT).
