# Migrations

`postgres/` is a verbatim copy of `server/channels/db/migrations/postgres/` from Mattermost v12.1.0
(commit `243e3d0135203ab9166fe8f2873b07f697b06fd4`), Copyright (c) 2015-present Mattermost, Inc., licensed under
AGPL-3.0.

- Never edit these files. Their numbers and names are what the Go server records in `db_migrations (Version, Name)`;
  a changed or renumbered file would make the two servers disagree about the schema.
- When we apply them, record each one in `db_migrations` exactly as the Go server does, so it does not run them again.
- These are only the SQL migrations. The Go server also runs data migrations in code (`server/channels/app/migrations.go`,
  24 of them), each marked done by a `*MigrationComplete` key in the `Systems` table. A database the Go server has
  already started on has them; a fresh one needs them too.
