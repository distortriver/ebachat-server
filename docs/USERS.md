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

- **Permanent user deletion** and **custom profile attributes:** ported, see `docs/ADMIN.md`.
- **Account-type switches** end in the provider's "not available" error after Go's user and password checks. No
  SSO provider or LDAP can be set up without a license.
- **`trigger-notify-admin-posts`** is Cloud's job. It answers Go's 403 unless `EnableAPITriggerAdminNotifications`
  is on.
- **Audits:** written as Go writes them, see `docs/ADMIN.md`.

## Sessions, tokens and updates as Go

- **The request's token** (`src/app.rs`, `ParseAuthTokenFromRequest`): the `MMAUTHTOKEN` cookie first (a quoted value
  unquoted, a malformed cookie skipped, an empty one meaning no token even with an `Authorization` header), then
  `Authorization: Bearer <t>` or `Token <t>` (the rest after the first space, untrimmed), then `?access_token=`;
  cut to 50 bytes.
- **Before the handler** (`web.Handler.ServeHTTP`): an unknown token is "session expired" with `MMAUTHTOKEN`
  removed (`Max-Age=0; HttpOnly`) where a session is required, no session elsewhere; a failed CSRF check of a
  cookie session is "session expired" with the cookie removed on every route that does not trust the requester
  (`APIHandler` ones included, so `POST /users/logout` answers 401 and keeps the session); the check passes when
  `X-CSRF-Token` equals the session's CSRF token, both possibly empty, as Go compares them.
- **`POST /users/logout`** removes `MMAUTHTOKEN` only, as `RemoveSessionCookie` does.
- **`PUT /users/{id}` and `PUT /users/{id}/patch`** answer the updated user as `UpdateUser` returns it to anyone
  allowed to edit it: `Sanitize({})`, no privacy settings applied (the patch route applied them before). A user of
  any login provider (LDAP and SAML included) cannot change its username on either route (409, `Field`
  `username`).
- **`POST /teams/{id}/members/ids`** applies the viewer's `GetViewUsersRestrictions`.
- **Switching an email account to GitLab:** the state keeps the email as typed, and the callback finds the user by
  `Email = lower(...)` and stores the provider's email in lower case, as `GetByEmail` and `UpdateAuthData` do.
- **Checked** against Go: an unknown cookie and header token, an empty cookie with a good bearer token, a quoted
  cookie, `bearer` with two spaces, a non-OAuth token in the query (API and web route), logout and a PUT with a
  failed CSRF check, with `X-Requested-With`, logout without a session; PUT and PATCH of another user by an admin.

## Go bugs fixed, not reproduced

- **View restrictions in team member lists** combine the teams and channels the viewer may see with OR, as
  `UserCanSeeOtherUser` does; Go's SQL filter joins both, which needs a user to be in a permitted team and a
  shared channel at once.
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
