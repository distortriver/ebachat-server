# Migrations

`postgres/` is a verbatim copy of `server/channels/db/migrations/postgres/` from Mattermost v12.1.0
(commit `243e3d0135203ab9166fe8f2873b07f697b06fd4`), Copyright (c) 2015-present Mattermost, Inc., licensed under
AGPL-3.0.

- Never edit these files. Their numbers and names are what the Go server records in `db_migrations (Version, Name)`;
  a changed or renumbered file would make the two servers disagree about the schema.
- The server applies them at startup as the Go server does (`src/migrate.rs`, after morph v1.1.0): under morph's
  lock (the `db_lock` row `mm-lock-key`, so a Go and a Rust server never migrate at once), Go's pre-migration first,
  then every file whose name is not in `db_migrations`, by version, each recorded there in the same transaction (or
  right after, for a file starting `-- morph:nontransactional`). `EBACHAT_MIGRATIONS=off` skips them.
  `tests/migrations.rs` checks that an empty database comes out with the very schema and rows Go leaves.
- These are only the SQL migrations. The Go server also runs data migrations in code (`server/channels/app/migrations.go`,
  24 of them), each marked done by a `*MigrationComplete` key in the `Systems` table. A database the Go server has
  already started on has them; a fresh one needs them too.

## Our own: `ebachat/`

Things the Go server does not see, which we need: indexes for our queries, statistics, and the like.

- Same file names as Mattermost's (`000001_name.up.sql`, `.down.sql` optional, never run by the server), same
  handling (one transaction per file, or `-- morph:nontransactional` as the first line for `CREATE INDEX
  CONCURRENTLY`, one statement per such file). They run after Mattermost's, under the same lock.
- Recorded in their own table, `ebachat_migrations (version, name)`, never in `db_migrations`: our numbers start at 1
  and never meet Mattermost's, and the Go server never sees our rows.
- Only what Go does not notice: never a table, column, constraint or row the Go server reads or writes. Name every
  object `ebachat_*`, write it idempotent (`IF NOT EXISTS`), and prefer `CONCURRENTLY` on big tables.
- A file is never edited or renumbered once released; to undo one, add a new migration that drops it.
- On a new Mattermost version, look whether its migrations now create an index we added under our own name; if so,
  drop ours with a new migration.
