# Admin and users leftovers: audits, permanent deletion, license, reports, logs, config, custom profile attributes

What the Rust server does in these areas, how it was checked against Go 12.1, and where it differs.

## Audits (`src/audit.rs`)

Go writes `Audits` rows from its handlers with `c.LogAudit` and `c.LogAuditWithUserId`: the URL path as the action,
the client's address (`TrustedProxyIPHeader`, else the peer), the session id, and the user (the session's, or the
one given, with `session_user=` appended). The Rust server writes the same rows at the same points.

- **Request context:** a middleware (`audit::layer`) keeps the request's address, path and method in a task-local for
  the handler. The server is served with `ConnectInfo`, so the peer address is known.
- **Call sites ported:** login (`attempt`, `failure`, `authenticated`, `success`), logout, password update and reset,
  MFA, roles, active, auth, user update and patch, profile image, email verification, terms of service, account type
  switch, device attach, sessions (revoke one, all of a user, all), personal access tokens (create, disable, enable,
  revoke, rotate), teams (patch, invite id, icon), channels (create, update, patch, privacy, delete, restore, members,
  move), roles (patch), config (`PUT /config`), incoming and outgoing webhooks, slash commands, OAuth apps, license
  (add, remove), product notices, onboarding, the first-admin-visit flag.
- **Not here:** the OAuth/SAML web pages and the guest magic link (the web routes' scope), `sendPasswordReset`'s
  `sent=` row (written when an email is sent: the notifications' scope), and licensed features (compliance,
  bookmarks, remote clusters, outgoing OAuth connections, brand image), which never reach their audit point here.
- **Checked:** the same 70 requests against Go and Rust (users, tokens, sessions, teams, channels, roles, config,
  hooks, commands, OAuth apps, license, notices), the new `Audits` rows of each normalized (ids by first appearance):
  identical, but for the rows of answers that differ on purpose (activating a user past Go's 250-user limit, and the
  missing `token_id`, where Go's row is written too).

## Server log (`src/logs.rs`)

- **Targets:** `LogSettings` as Go reads it: the console (standard error) at `ConsoleLevel`, JSON or plain, and with
  `EnableFile` the file `mattermost.log` in `FileLocation` (empty: the `logs` directory found as Go's `FindDir`
  finds it) at `FileLevel`, JSON or plain. Reconfigured on every config change.
- **Format:** Go's: `{"timestamp":"2026-11-20 12:00:00.123 +02:00","level":"info","msg":"...","caller":"file.rs:42",...}`
  (logr's JSON formatter field order, field names that collide prefixed with `_`, `MaxFieldSize`), or the plain
  `level [timestamp] msg caller="..." k=v` line.
- **What is logged:** every error answer as Go's `LogErrorByCode` (4xx at debug, 501 at info, the rest at error) with
  the request's `path`, `request_id`, `ip_addr`, `user_id`, `method`; client logs; the license and permanent
  deletion messages; "Server is listening on ...".
- **Routes:** `GET /logs` (the last lines, a page of `logs_per_page` walked back from the end, as Go does, with
  each line's leading newline), `POST /logs/query` (`LogFilter` with Go's date layout and level filter, lines parsed
  as JSON under `default`), `GET /logs/download` (the whole file as an attachment), `POST /logs` (client messages).
  The file must be inside the logging root (`MM_LOG_PATH` or the `logs` directory), as Go checks.
- **Go bug fixed:** none. **Differs:** no rotation (Go's lumberjack rotates at 100 MB); no advanced logging targets
  (a licensed feature).

## Permanent deletion (`src/permanent.rs`)

`DELETE /users/{id}?permanent=true` (`EnableAPIUserDeletion`), `DELETE /teams/{id}?permanent=true`
(`EnableAPITeamDeletion`) and `DELETE /channels/{id}?permanent=true` (`EnableAPIChannelDeletion`) do what Go's
`PermanentDeleteUser`, `PermanentDeleteTeam` and `PermanentDeleteChannel` do, table by table and in Go's order:

- **User:** deactivated first, then sessions, access tokens (and their sessions), OAuth access data, incoming and
  outgoing webhooks, commands, preferences, channel and group memberships, posts (replies first, their threads'
  reply counts and participants updated; then root posts with their threads, reactions, read receipts and replies,
  10 batches of 1,000 at most), reactions (posts' `HasReactions` again), scheduled posts, drafts, the bot row, file
  infos and their files, the profile image directory, the user, audits, team memberships.
- **Channel:** posts in batches of 500 with threads, reactions, temporary posts and read receipts; members; webhooks;
  persistent notifications; the channel and its `PublicChannels` row; its access control policy (moved to the
  history); `channel_deleted`.
- **Team:** marked deleted, then every open, private and group channel (and "space" channels) as above, members,
  commands, the team, its access policy; `delete_team`.
- **Checked:** the same user (posts, a reply in another's thread, a reply from another under the user's post, a
  reaction, preferences, a draft), team (private channel with posts, reaction, webhook, command) and channel built
  through Rust and deleted once by Rust and once by Go: every count left in every table, the thread's reply count and
  participants, and `HasReactions` are the same. Go, too, leaves the user's `ThreadMemberships` row on another's
  thread and the team's sidebar categories.

## License (`src/license.rs`)

- **Verification:** as `utils.LicenseValidator` (not enterprise code): base64 (line breaks skipped), trailing zero
  bytes dropped, the last 256 bytes an RSA PKCS#1 v1.5 signature over SHA-512 of the rest, checked against the public
  key of the service environment (production, or test for `dev`/`test`) that Go embeds (`data/license/`), and against
  the other one to tell a license of the wrong environment.
- **Storage:** as `SaveLicense`: the user-count and expiry checks, the `Licenses` row (kept if it exists),
  `ActiveLicenseId`, `HostedPurchaseNeedsScreening` dropped, caches cleared, `license_changed` with the sanitized
  client license. `DELETE /license` sets `ActiveLicenseId` to `""`. At start the license comes from `MM_LICENSE`, the
  database, or the license file, as `LoadLicense`.
- **Routes:** `POST /license`, `POST /license/preview`, `DELETE /license`, `GET /license/client` (full with
  `read_license_information`, sanitized otherwise).
- **Checked** with invalid and forged licenses only (not base64, too short, bad signature, line breaks, empty, no
  file, a non-file field, not multipart, a non-admin): status and error id as Go for add and preview; remove and the
  client license as Go.
- **Differs:** a valid license would be stored and served by `/license/client`, but the licensed features stay
  unported: the server keeps answering them as Team Edition.

## User reports

`GET /reports/users` takes `date_range` (`last_30_days`, `previous_month`, `last_6_months`): post statistics count
only the `PostStats` days in range (local dates, the start inclusive, the end exclusive, months added as Go's
`AddDate`). Go 12.1 has no `start_at`/`end_at` on this route.

## Config

- **Validation added:** OpenID additional scopes (RFC 6749 tokens, 1,024 at most), content flagging (events,
  targets, reviewers told of a flag, reasons), delivery tracking toggles, AI recap limits, the retired feature flags
  (`AppsEnabled`, `MoveThreadsEnabled`), and LDAP filters compiled as `go-ldap`'s `CompileFilter` does (with
  `LdapSettings.Enable`).
- **Key order:** the config file and a database config are written as Go's `MarshalIndent` writes them
  (`model.Config`'s field order, four-space indent, `<`, `>`, `&` escaped); the answers of `PUT`/`PATCH /config` are
  in that order too. `GET /config` answers a map, its keys sorted, as Go's `FilterConfig` does.
- **Not ported:** the advanced logging targets' own checks (`cfg.IsValid(MlvlAll)`); only that the JSON is an object.

## Custom profile attributes (`src/cpa.rs`)

The fields are the user fields of the `access_control` property group, which an unlicensed Go server's property
service refuses whenever it finds one (`LicenseCheckHook`):

- `GET /custom_profile_attributes/fields`: `[]`, or `app.property.license_error` (403) when the group has user fields;
- `POST .../fields`: the body, `manage_system`, then the license error;
- `PATCH` and `DELETE .../fields/{id}`: 404 for a field that is not there, the license error for one that is;
- `GET /users/{id}/custom_profile_attributes`: `{}`, or the license error when the user has values;
- `PATCH /custom_profile_attributes/values` and `PATCH /users/{id}/custom_profile_attributes`: Go's checks (target
  access, empty, more than 100, bad ids), 404 when a field is missing, the license error when they all exist.

Checked against Go with a real field row in the group (test data, removed afterwards).
