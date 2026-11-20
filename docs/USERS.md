# Users, bots, tokens, OAuth apps: where the Rust server differs from Go

The `users`, `bots`, `oauth` and `terms_of_service` routes of `docs/map/routes.tsv` are all served.

- **Code:**
  - `src/user_admin.rs`: update, deactivate, roles, auth, MFA, sessions, audits, reset/verify tokens, guests, terms
    of service, account switching, the unlicensed answers.
  - `src/tokens.rs`: personal access tokens.
  - `src/bots.rs`: bots.
  - `src/oauth.rs`: OAuth apps.
  - `src/terms.rs`: terms of service.
- **Checks:**
  - `tools/diff/routes.py --area users,bots,oauth,terms_of_service`.
  - Request flows run against both servers, with users made through Rust because of Go's user limit: creating,
    deactivating, roles, PUT user, sessions, tokens (enable, disable, rotate, revoke, use), bots (create, the
    DM post, patch, assign, disable, convert both ways), and OAuth apps (CRUD, regenerating the secret, public
    clients).

## Answered as an unlicensed Go server

These answer with Go's license or "not available" error:

- data retention policies;
- groups;
- demoting to guest;
- creating terms of service;
- LDAP/SAML migration;
- CWS login;
- `login/type` (a bare 404);
- SSO code exchange (410);
- session attributes (501);
- dynamic client registration (Go's RFC 7591 error body).

## Not ported

- **Permanent user deletion** (`DELETE /users/{id}?permanent=true` with `EnableAPIUserDeletion`) answers 501. It
  is off by default, and Go then answers 401, as Rust does.
- **Custom profile attribute values** (`PATCH /users/{id}/custom_profile_attributes`): Go's checks are ported up to
  the field lookup. The database has no user attribute fields, so every valid request ends in Go's
  `app.property_field.not_found.app_error`.
- **Account-type switches** end in the provider's "not available" error after Go's user and password checks. No
  SSO provider or LDAP can be set up without a license.
- **`trigger-notify-admin-posts`** is Cloud's job. It answers Go's 403 unless `EnableAPITriggerAdminNotifications`
  is on.
- **Audits:** Go writes `Audits` rows (`LogAudit`). The Rust server writes none, so `GET /users/{id}/audits` shows
  only what Go wrote.

## Go bugs fixed, not reproduced

- **Missing `token_id` on revoke, disable or enable of a token:** Go carries on without a `return`. It then answers
  with a later error (a 404 for the token `""`). Rust answers the missing parameter (400).
- **A bot converted from a user owns itself.** With `DisableBotsWhenOwnerIsDeactivated`, deactivating it makes Go
  disable the bot again from inside its own deactivation, without end, until the goroutine's stack overflows.
  Rust skips the bot itself. **Do not disable such a bot on Go:** assign it to another owner first.
- **Converting a bot to a user:** Go leaves the user cached as a bot. Rust drops the cache entry.
- **`reset_failed_attempts`:** Go leaves the cached user with its old attempt count. Rust drops the cache entry.
- **MFA activation:** Go answers a code that is not six digits with a 500. Rust answers it as a bad token (401).

## Shared code changed

- **`Body<T>` (`src/app.rs`):** a JSON list of strings is read as Go's `SortedArrayFromJSON`, which every Go
  handler with such a body uses. Anything unreadable is `api.payload.parse.error`; `null` is an empty list. Other
  bodies are unchanged.
- **New helpers in `src/app.rs`:**
  - `decode_json`: Go's `json.Decoder` into a struct, with the parameter name for the error.
  - `map_from_json`: Go's `model.MapFromJSON`.
- **`POST /users/{id}/typing` (`src/posts.rs`):** Go's order is ported: body, then `manage_system` for another
  user's typing, then the user's (not the session's) `create_post`. The `EnableUserTypingMessages` check is gone;
  Go's API does not make it.
- **`GET /users/{id}/preferences`:** `null` when the user has none, as Go answers.
- **`POST /users/{id}/image`:**
  - a body that is not multipart is Go's 500 `parse` error;
  - the permission is `SessionHasPermissionToUserOrBot`.
