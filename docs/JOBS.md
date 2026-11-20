# Background jobs

What an unlicensed Go 12.1 server runs in the background, and how the Rust server runs it. Code: `src/jobs.rs`
(the job server and `/jobs` routes), `src/job_workers.rs` (workers, schedulers, recurring and startup tasks),
`src/extract.rs` (content extraction), `src/bulk_export.rs` (bulk export, `/exports`, `/imports`),
`src/bulk_import.rs` and `src/import_functions.rs` (bulk import).

## The job server

As Go's `channels/jobs`:

- **Rows:** `Jobs` with Go's types, statuses, progress and data. `Data` is `null` for a job created without data,
  as Go's `json.Marshal(nil map)`. Status changes are Go's optimistic updates (`UPDATE ... WHERE Status = ...
  RETURNING`), so Go and Rust can share one database: a pending job is claimed by whichever server gets there
  first, never by both.
- **Watcher:** every 15 s (after a random start delay under 15 s), pending jobs oldest first are handed to their
  worker if it is enabled and idle. One job per worker type at a time, as Go's unbuffered channels.
- **Schedulers:** checked every minute; a due scheduler creates its job and sets its next time (periodic: now, a
  jitter under 2 s and the period; daily: the next local `HH:MM`). A config save sets every next time again.
- **`JobSettings.RunJobs` / `RunScheduler`** start the workers and the schedulers. Single node: always leader.
- **`job_updated`:** sent on every status change, with the job as JSON text, to sessions holding the type's read
  permission (system admins when the type has none). The hub gained `required_permissions` for this.
- **Simple workers** record success as Go: progress 100, then `success`; on error `progress = -1` and
  `data.error = "Error during job execution. — <cause>"`. The plugins and migrations workers leave progress at 0.

Without a license `POST /jobs` accepts the types Go registers a worker for; licensed ones (Elasticsearch, LDAP,
compliance, access control) answer `model.job.is_valid.type.app_error` as Go.

## Routes

| Route | |
| --- | --- |
| `GET /jobs`, `GET /jobs/{id}`, `GET /jobs/type/{type}` | `admin.rs`; `job_type` must be one of `AllJobTypes`; `team_id`/`policy_id` filters as Go |
| `POST /jobs` | creates the pending job (201), the worker picks it up |
| `POST /jobs/{id}/cancel` | pending → canceled, in progress → cancel requested |
| `PATCH /jobs/{id}/status` | `IsValidStatusChange` unless `force`; pending, cancel requested, canceled |
| `GET /jobs/{id}/download` | message exports only: they need a license |
| `GET /exports`, `GET|DELETE /exports/{name}.zip`, `POST .../presign-url` | the export directory (presign: Cloud only, 403) |
| `GET /imports`, `DELETE /imports/{name}.zip` | the import directory, without incomplete uploads |

## Workers and schedulers without a license

| Type | Scheduler | Worker does |
| --- | --- | --- |
| `migrations` | every minute until done | advanced permissions phase 2 (team and channel members' scheme roles), 100 rows a second; nothing once `migration_advanced_permissions_phase_2` is in `Systems` |
| `plugins` | 24 h | expired plugin KV rows deleted |
| `expiry_notify` | 10 min, with `ExtendSessionLengthWithActivity` | a `session` push to mobile sessions expired in the last hour, `ExpiredNotify` set |
| `product_notices` | `NoticesFetchFrequency`, and once at startup | notices fetched (ETag cache), view states of notices no longer listed deleted |
| `import_delete`, `export_delete` | 24 h, with a directory and retention | files past retention removed (imports: their upload session or FileInfo too) |
| `active_users`, `mobile_session_metadata` | 10 min / 24 h, with metrics | nothing to report without a license's metrics |
| `extract_content` | 1 h, with `ExtractContent` (`catchup`) | text of files without content from the last 24 h (or `from`..`to`); `processed`, `errors` in data |
| `install_plugin_notify_admin` | 24 h | users' feature and plugin requests posted to system admins by the system bot, at most every 14 days |
| `cleanup_desktop_tokens` | 1 h | desktop tokens older than 5 min |
| `cleanup_expired_access_tokens` | 1 h | expired personal access tokens and their sessions deleted; owners told by the system bot (expired in the last week) |
| `notify_expiring_access_tokens` | 1 h | owners warned at 7, 3 and 1 day(s) before expiry, once per bucket (`LastNotifiedAt`) |
| `refresh_materialized_views` | daily at `RefreshPostStatsRunTime` | `posts_by_team_day`, `bot_posts_by_team_day`, `file_stats`, `poststats` refreshed |
| `delete_expired_posts` | `BurnOnReadSchedulerFrequencySeconds`, with burn on read | expired burn-on-read posts scrubbed and deleted, `deleted_post_ids` in data |
| `delete_empty_drafts_migration`, `delete_orphan_drafts_migration`, `delete_dms_preferences_migration` | created at startup unless done | Go's batch migrations, a batch a second; key in `Systems` when done |
| `export_process` | (created by `POST /jobs`) | bulk export, below |
| `import_process` | (created by `POST /jobs`) | bulk import, below |
| `resend_invitation_email` | (created by team invites) | after 48 h, invitations of emails that have not joined resent; `Systems` row of the job removed |
| `hosted_purchase_screening` | never (Enterprise builds) | screening flag dropped after 3 days |
| licensed types | never | registered, never enabled: their jobs stay pending |

## Recurring tasks and startup (`server.go`)

- **Sessions:** expired ones deleted daily, 1000 at a time.
- **Tokens:** every token older than 48 h but CWS access tokens, hourly.
- **Command webhooks:** older than 30 min, hourly.
- **Jobs:** finished ones older than `CleanupJobsThresholdDays`, daily, when it is not negative.
- **Configurations:** old inactive rows (the newest 5 kept), daily, for a config in the database with
  `CleanupConfigThresholdDays` set.
- **Guests:** deactivated at startup when guest accounts are off (or only magic-link guests when magic links
  are off), and when a config save turns them off: sessions revoked, `userDeactivated`, caches cleared,
  `guests_deactivated` sent.
- Already ported before: DND expiry (`status.rs`), post reminders (`reminders.rs`). Scheduled posts, the health
  check, the license expiry check and Cloud's tasks do nothing without a license.

## Content extraction

`extract.rs` ports `docextractor`: documents (docx, pptx, odt as their XML text the way `docconv` reads it, pdf
through `pdftotext` as `docconv` runs it), archives (zip, tar, tar.gz: the entry names), plain text (the first KB
must be printable UTF-8; 1 MB kept). Text is saved in `FileInfo.Content` (1 MB at most) by the hourly job and,
as Go, in the background after each upload when `ExtractContent` is on.

## Bulk export

`export_process` writes `ExportSettings.Directory/<job id>_export.zip`: `import.jsonl` (deflated) and, as asked,
attachments, custom emoji and profile pictures under `data/` (stored). The JSONL is Go's: line types and field
order, `omitempty` and `null`s as Go's pointers and nil slices give them, Go's HTML escaping (`<`). Options
from the job data: `include_attachments`, `include_archived_channels`, `include_profile_pictures`,
`include_roles_and_schemes`, `include_custom_emoji`, `team_name`, `channel_name` (scoped exports, with
memberships synthesized for authors who left). Progress counters (`teams_exported`, `users_exported`, ...) are
written to the job's data as Go does.

## Bulk import

`import_process` (`bulk_import.rs`, `import_functions.rs`) is Go's `BulkImportWithPathAndOpts`:

- **The file:** `import_file` in `ImportSettings.Directory` (or a local path with `local_mode`), a zip with the
  JSONL at its root and the attachments, profile images and emoji under `data/`. A zip of the local store is read
  in place, at offsets; from S3 it is read whole. Removed after a successful import, as Go does.
- **Uploads:** `POST /uploads` with `type: import` (system admins), then the parts; the file lands as
  `<upload id>_<name>` in the import directory (`GET /imports` lists it). As `mmctl import upload`.
- **Job data:** `workers` (default: the CPUs), `extract_content`, `destination_team_name`,
  `destination_channel_name`, `skip_preflight`, `imported_users`; on failure `line_number`; for JSONL of 100 MB
  and more, `checkpoint`/`checkpoint_file`/`total_lines` after each segment, and a failed job resumes from its
  checkpoint (post lines up to it skipped).
- **The lines:** `version` (first, version 1, with the export scope in `info.additional`), then `role`,
  `scheme`, `team`, `channel`, `user` (team and channel memberships with their roles, notify props, counts,
  favorites, theme and the other preferences, custom status, profile image), `bot`, `emoji`, `post` (replies,
  reactions, attachments, flags, pins, thread followers), `direct_channel` (members or participants, favorites,
  shown by, header) and `direct_post`. Each type is validated as Go's `imports/import_validators.go`, with its
  error ids, then written as `import_functions.go` writes it: straight to the stores (no websocket events but where
  Go calls app functions: team and channel creation and updates, user updates and roles, bots, profile images).
  Lines already imported are updated, not duplicated (posts are found by channel, time and message).
- **Segments and workers:** a change of line type waits for the workers of the previous type; posts go in
  batches of 1000 per worker. The first error that stops the import is the job's error, with its line number (0
  where Go cannot tell). Errors Go goes on after (large images, direct channels of too few or too many members,
  and in scoped imports existing usernames and emails and the last-admin refusal) are logged.
- **Scoped imports** (an export of some teams or channels): users missing here are created, active or
  inactive as `imported_users` says (required), SSO users are created before the main pass and matched by auth
  data, the SSO providers they need are checked (`skip_preflight`), and teams and channels can be renamed with
  `destination_team_name` and `destination_channel_name`.
- **Dry run** (validation only) is there for callers that ask for it; the job never does, as in Go.

## Deviations

- **Security bulletins (`runSecurityJob`) are not fetched.** Go sends the server id and counts to
  `securityupdatecheck.mattermost.com` and mails admins Mattermost's bulletins for its own Go code; they do not
  apply to this server.
- **Product notices are fetched but not shown:** they target Mattermost versions; `GET /system/notices` keeps
  answering none (Go answers none on the stand too).
- **Bulk import:**
  - a line that is not JSON fails with Go's error id and line number, but the decoder's own words are serde's
    (Go's for a cut or empty line: `unexpected end of JSON input`); JSON keys are matched exactly (Go's decoder
    ignores their case).
  - the uploads of import files were stored under `%2E/import/` (the `./import` of the config, uncleaned) and so
    never found by the job or `/imports`; their path is cleaned as Go's `filepath.Join` cleans it.
  - a bot's welcome post (`CreateBot`) no longer marks the bot's direct channel viewed: Go's `CreatePostAsUser`
    skips that for posts `from_bot`.
- **Resent invitations need the email sender** (the notifications part): until it is wired in, a due resend job
  fails with `api.team.invite_members.unable_to_send_email.app_error`.
- **Session expiry pushes** are sent by `job_workers.rs` itself (push proxy `send_push`, `REMOVE` handled); to be
  folded into the push sender of the notifications part.
- **Extraction:** `.doc`, `.rtf` and `.html` documents fall through to plain text (Go needs `wvText`/`unrtf`,
  which it does not ship; `html` goes through `docconv`'s readability, not ported); a pdf without `pdftotext`
  gives no text (Go falls back to a Go PDF reader). Text cut at 1 MB stays valid UTF-8 (Go may cut inside a
  character and then fail to save it).
- **`job_updated` to non-admins** with the required permission works through each connection's session.
- `LOG_JOBS=1` logs each job the Rust server ran.

## Verified against Go

- **Job routes:** `/jobs` create (bad bodies, unknown and licensed types, permissions), cancel, status changes
  with and without `force`, download, `GET /jobs?job_type=`: 32/32 answers as Go, as admin and as a user.
- **Workers:** pending jobs were inserted as Go's schedulers insert them and run by the shared Go server or by
  Rust, whichever claimed them (`LOG_JOBS`). For `cleanup_desktop_tokens`, `plugins`,
  `cleanup_expired_access_tokens`, `notify_expiring_access_tokens`, `refresh_materialized_views`,
  `product_notices`, `import_delete`, `export_delete`, `install_plugin_notify_admin`, `hosted_purchase_screening`,
  `extract_content`, `mobile_session_metadata` and `delete_expired_posts`, the `Jobs` rows (status, progress,
  data, error text) and the effects (rows deleted, the system bot's DM) were the same: 13/13.
- **Bulk export:** a team-scoped export (`compat`, with roles and schemes, attachments, profile pictures and
  custom emoji) by Go and by Rust: the same zip entries and methods, the same 126 JSONL lines byte for byte (but
  `created`), the same job data. A full export (direct channels and posts) was not compared: Go takes about
  25 minutes over the stand's 42k posts.
- **Bulk import:** on a throwaway database (so that no other server claims the jobs), Go 12.1 and Rust took
  turns importing the same zip under different names: a custom role, a team and a channel scheme, two teams, open,
  private and archived channels, four users (password, generated password, guest, deactivated with GitLab auth;
  team and channel roles with a custom one, counts, notify props, favorites, every preference, custom status,
  profile image), a bot with an avatar, a custom emoji, posts with replies, reactions, attachments, flags, a pin
  and thread followers, a direct and a group channel and their posts. The rows written to 19 tables (roles,
  schemes, teams, channels, public channels, users, bots, team and channel members, member history, preferences,
  posts, threads, thread memberships, reactions, file infos, emoji, sidebar categories, sessions) were the same,
  126/126, ids and times aside; so were they after importing the same file again (the update paths). 22 broken
  files (no or wrong version, bad JSON, unknown and null lines, invalid teams, users, guests, schemes, roles,
  emoji, missing teams, channels and users, dangling reactions, too few direct members): the same job status,
  error text and `line_number`, 21/22 identical (for the bad JSON line only the decoder's words differ, above).
  An import file uploaded through `/uploads` and imported: the same answers and `FileInfo` row. Random values
  (password hashes, invite ids) and a guest deactivated by the other server's startup (`DeactivateGuests`, guest
  accounts are off on the stand) were left out of the comparison.
