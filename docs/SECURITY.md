# Security review fixes

Seven findings of a security review of the Rust server, each fixed to behave as Go 12.1 does. Each was checked
against Go with real requests: my own Go instance (`dev/bin/mattermost`, a config copy with MFA on, `AllowCorsFrom`
set, the mail server pointed at a local fake SMTP server) and my own Rust instance with the same settings, on the
stand's database. Throwaway data was removed afterwards (see the end).

## 1. SMTP command and header injection (high)

- **Cause:** `mail.rs` took an address apart by hand (`<...>`, a comma count) and accepted CR and LF, and
  `Conn::cmd` wrote every line as it came. An invited address like `a@b.com\r\nRCPT TO:<x@y>` (from
  `POST /teams/{id}/invite/email` or the `resend_invitation_email` job) became a second SMTP command, and the
  `To:` header was built from the same value.
- **Fix:** `parse_address_list` is a port of Go's `net/mail` address-list parser (addr-spec, phrase, quoted
  strings, comments, groups, domain literals, RFC 2047 words), with Go's error texts; `validateSingleAddress`
  uses it. `Conn::cmd` refuses any line with CR or LF (`smtp: A line must not contain CR or LF`, `net/smtp`'s
  `validateLine`), so no value (recipient, sender, host name) can carry a command. Go hands the raw recipient to
  `Rcpt`, so a value with CR or LF inside a parenthesised comment (which parses) fails there: Rust fails it the
  same way, with the same error.
- **Answer of the invite route:** as Go: without `graceful`, 200 OK and nothing sent; with `graceful`, the address
  with `api.team.invite_members.unable_to_send_email.app_error`. (The errors inside that list no longer carry a
  request id, as Go's.)
- **Verified:** `mail::tests::parses_addresses_like_go` (34 inputs, expectations printed by Go's `ParseAddressList`),
  `mail::tests::no_command_injection` (a fake server sees no injected command). Against Go: the invite route, plain
  and graceful, with three injected addresses and one good one: same answers, no `RCPT` for the injected ones on
  either server, one for the good one.
- **Deviation:** the `RCPT` command carries the parsed address; Go sends the raw value (`RCPT TO:<Name <a@b>>` for
  a value with a display name). Callers only pass bare addresses, so nothing differs in practice.

## 2. SSRF through `GET /api/v4/redirect_location` (and every untrusted client)

- **Cause:** the untrusted HTTP clients (`link_client`, `integration_client`, `no_redirect_client`) checked
  addresses only in their DNS resolver. hyper connects to an IP-literal host (`http://127.0.0.1/`,
  `http://[::ffff:127.0.0.1]/`, `http://2130706433/`) without asking the resolver, so the route fetched internal
  addresses. Go checks every address it dials (`dialContextFilter`).
- **Fix:** a connector layer (`ConnectGuard` in `opengraph.rs`) on all three clients, below the resolver, so no
  caller has to remember a pre-check:
  - a literal host is checked before dialling (no TCP connection is made to a refused address);
  - after connecting, the address really connected to (`HttpInfo::remote_addr`, the proxy's when there is one, as
    Go's dialer sees it) is checked again;
  - a host listed by name in `AllowedUntrustedInternalConnections` passes, as Go's `allowHost`;
  - errors are Go's (`address forbidden, you may need to set AllowedUntrustedInternalConnections ...: IP x is in a
    reserved range ...`).
  
  The handler also checks the URL first (`untrusted_url_allowed`). Go's answer for a refused URL is
  `{"location": ""}` (cached an hour like any failure); so is Rust's.
- **Verified:** `opengraph::tests::guard_refuses_internal_addresses` (a literal is refused without a connection, a
  name resolving to loopback is refused after connecting, an allowed CIDR or host goes through). Against Go:
  `redirect_location` for `127.0.0.1`, `[::ffff:127.0.0.1]`, `localhost`, `0.0.0.0`, `[::1]`, the LAN address and
  `2130706433`, each pointing at a local server that redirects: `{"location": ""}` on both.
- **Note:** reqwest does not name its connector request type, so the guard reads the host from its `Debug`
  (`Unnameable(<uri>)`, tested); if that ever fails, no host is allowed by name (it fails closed).

## 3. The server's own addresses (`IsOwnIP`)

- **Cause:** `Allowed::ip` only refused reserved ranges. Go's `checkInternalIP` also refuses any address of the
  host's own interfaces (a public address of the server itself).
- **Fix:** `is_own_ip` lists the interfaces with `getifaddrs` (the `libc` crate, MIT/Apache) on every check, as Go
  does; an own address passes only inside an allowed CIDR; if the interfaces cannot be listed, nothing is allowed
  (`unable to determine if IP is own IP: ...`). IPv4-mapped IPv6 addresses are compared as IPv4. Go's messages:
  `IP x is a self-assigned IP and not in AllowedUntrustedInternalConnections`.
- **Verified:** `opengraph::tests::internal_ips` (reserved, own, mapped, allowed CIDR, interface failure, the real
  loopback interface). A public own address could not be set up on the stand, so it is checked by the unit test.

## 4. Websocket origin and query-string token

- **Cause:** `GET /api/v4/websocket` upgraded any request; Go's upgrader checks `App.OriginChecker`. It also
  accepted a session token in `?access_token=` for any session.
- **Fix:** `hub::connect` does what `APIHandlerTrustRequester` and gorilla's `Upgrader.Upgrade` do, in their order:
  - the session of the token: a token in the query string for a non-OAuth session is
    401 `api.context.token_provided.app_error`; a token that is not found is no session, and the client may still
    authenticate on the socket, as in Go (it used to close the socket);
  - `Connection: upgrade`, `Upgrade: websocket`, `GET`, `Sec-WebSocket-Version: 13`;
  - the origin, by `App.OriginChecker`:
    - with `AllowCorsFrom`, those origins plus the site URL, or any for `*`;
    - without it, the origin's scheme and host must be the site URL's (the request's `Host` when there is no
      site URL), case-insensitive;
    - no `Origin` header is fine, `null` is not;
  - a 16-byte `Sec-WebSocket-Key`.
- **Answer:** a refusal is answered as Go answers it: gorilla's `http.Error` (403 for the origin, 400 or 405 for the
  rest, `text/plain`, `Sec-Websocket-Version: 13`, the status text), followed in the same body by the handler's
  `api.web_socket.connect.upgrade.app_error` JSON ("URL Blocked because of CORS. Url: <origin>", 400 inside).
- **Verified:** against Go 8065 (no `AllowCorsFrom`): 17 raw upgrade requests (no origin, the site URL in both
  cases, `null`, another host, another scheme, an empty origin, a bad URL, no upgrade headers, a bad version, a bad
  key, header token, unknown token, query token): status, headers and body identical. Against my Go with
  `AllowCorsFrom`: the allowed origin, the site URL, another host, `null`, none, empty: identical.

## 5. STARTTLS downgrade

- **Cause:** the auth decided "TLS" from `ConnectionSecurity`, not from the stream. A server (or a man in the
  middle) that did not offer STARTTLS left the session in plain text, and PLAIN or LOGIN then sent the
  credentials in the clear. Rust also only sent STARTTLS when the server offered it.
- **Fix:** `Conn.tls` is true only for a TLS connection or a STARTTLS that went through. Like `net/smtp`, STARTTLS
  is sent whenever `ConnectionSecurity` is `STARTTLS` and its error ignored; then both PLAIN (`plainAuth.Start`)
  and LOGIN (Mattermost's `loginAuth.Start`) refuse a session that is not really TLS with
  `unencrypted connection`, after a `QUIT` as `Client.Auth` does. Go passes `host:port` as the server name, so
  `net/smtp`'s localhost exemption never applies; Rust has none either.
- **Verified:** `mail::tests::no_credentials_without_tls`. Against Go: both servers' mail settings pointed at a fake
  server that offers `AUTH PLAIN LOGIN` and answers STARTTLS with 454, then `POST /email/test`: with `STARTTLS` and
  with no security, both answer 500 `Connection unsuccessful: authentication failed: unencrypted connection`, and
  the fake server sees the same commands from both (`EHLO`, `STARTTLS`, `QUIT`; `EHLO`, `QUIT`) and no credentials.

## 6. MFA lockout bypass and code replay

- **Cause:** the account switch (`POST /users/login/switch`, which needs no session) checked the password with
  `double_check_password`, which reset `FailedAttempts` to 0 before the MFA code was looked at: wrong MFA codes were
  never counted, so MFA could be guessed without limit. Login and switch both checked codes with a plain TOTP check,
  so a code could be used again within its window.
- **Fix:** `App::check_password_and_all_criteria` ports `CheckPasswordAndAllCriteria`:
  - preflight: deactivated, bot, attempts;
  - a lockout slot claimed with `TryIncrementFailedPasswordAttempts` (atomic);
  - the password: wrong or empty keeps the slot; an old hash is migrated with Go's `UpdatePassword`;
  - `CheckUserMfa`: a wrong code keeps the slot, a missing one gives it back;
  - the count reset to 0 only once both pass; then postflight (email verified).
  
  Login now runs Go's `authenticateUser` around it: LDAP users get 501, other sign-in methods 400 with the
  method's name, then every error as 401. The switch to SSO or LDAP uses it as Go's `SwitchEmailToOAuth` and
  `SwitchEmailToLdap` do.
- **MFA codes** (`mfa.ValidateToken`, dgoogauth): a trimmed code of six digits (eight: a scratch code, never good),
  a window of three steps, and `DisallowReuse` from `MfaUsedTimestamps`:
  - a step already used is refused;
  - the used steps are stored as Go stores them (a JSON array of strings, steps before the window dropped,
    `UpdateAt` set);
  - not a code is 400 `mfa.validate_token.authenticate.app_error`, a wrong or reused code 401
    `api.user.check_user_mfa.bad_code.app_error` (on login every error is 401).
  
  MFA activation uses the same check and also stores strings (it stored numbers, which Go cannot read). Generating
  a secret and deactivating clear the used steps and set `UpdateAt`, as `UpdateMfaSecret` does.
- **Verified:** `user_admin::tests::mfa_codes_like_dgoogauth`, `rfc6238_sha1`. Against Go, on a throwaway user with
  MFA (created and activated through Rust, since Go refuses users past 250), each server in a fresh 30 s window:
  - the sequence: login with no code, wrong code, wrong password, garbage code, good code, the same code again, then
    the switch with no code, a wrong code and a used code;
  - the lockout: ten wrong codes, then a good one and none.
  
  All 22 steps gave the same status, error id, `FailedAttempts` and `MfaUsedTimestamps` (relative steps, strings)
  on both. The user was then deleted permanently through Go.
- **Kept deviation:** activating MFA with a code that is not six digits answers 401 `mfa.activate.bad_token`, where
  Go answers 500 (as before).

## 7. `PublicLinkSalt` never generated

- **Cause:** `data/config_defaults.json` holds `""`, and nothing replaced it, so public file links were signed with
  an empty salt.
- **Fix:** as `FileSettings.SetDefaults` of an existing config (`Store.Load`, `Store.Set`): a missing or empty salt
  becomes `NewRandomString(32)` on load, reload and save, and is written to the config source (load already
  persists; a reload that made one persists it, as `Store.Load` writes back what changed). It is the only secret
  Go 12.1's `SetDefaults` generates (no `AtRestEncryptKey` or other `NewRandomString` default exists in 12.1).
  The shared `dev/run/go/config.json` already has Go's salt and is unchanged.
- **Verified:** `config::tests::link_salt_is_generated_and_kept`. Against Go: config copies with `""` got a new
  32-character salt in the file at start on both servers; `PUT /config` with an empty salt answered 200 with the
  masked salt on both, and both files held a new 32-character salt.
- **Deviation:** Go generates only when the config has a `SiteURL` key (`isUpdate`); setting a config without one
  leaves the salt empty until the next load. Rust always generates one.

## Also fixed on the way

- Login with an old password hash migrates it through `store_password`, which also sets `LastPasswordUpdate` as
  Go's `UpdatePassword` does.
- The SSO criteria check (`check_user_all_authentication_criteria`) tests in Go's order: deactivated, bot, attempts.
- `tools/diff/caches.py` tells the Rust server by its counters route instead of port 8066, so it runs against any
  port.

## Found, not changed

- `POST /email/test`: Go checks the posted settings (server set, password) but sends with the server's own mail
  settings (`MailServiceConfig()`); Rust sends with the posted ones. That is the notifications area, not changed
  here.
- `POST /users/password/reset/send` (Go and Rust alike): an unknown email answers 200, a known one 200 too, but
  when the mail cannot be sent a known email answers 500 (`api.user.send_password_reset.send.app_error`): with
  mail sending broken or off, the route tells which emails have accounts. The recovery token is written anyway.
  Kept as Go: hiding the failure would hide a broken mail setup from the user. (`tools/diff/scen_auth.py`)
- Cookie sessions pass the CSRF check with `X-Requested-With: XMLHttpRequest` alone unless
  `ExperimentalStrictCSRFEnforcement` is on (Go's deprecated fallback, MM-13657); kept for the clients that rely
  on it. A session without a `csrf` prop (one not made by a login) passes with no header at all, as in Go; such
  tokens never come as cookies from the server.

## Clean-up

The invite tests left `Tokens` rows and `resend_invitation_email` jobs for the test addresses on both servers;
they were deleted. The MFA user was deleted permanently through Go. My Go instance ran for about fifteen minutes
on the shared database, with its job schedulers.

## Websocket frames re-inserted a stale session into the cache (second review, fixed)

- **Was:** the socket loaded its session once at connect and handed that copy to every frame;
  `user_update_active_status` (`touch_session`) and `user_typing` (`extend_session_if_needed`) put it back into
  the session cache. A demoted admin kept `system_admin` on REST, and a revoked session could be re-inserted in the
  moment before its socket closed. Go re-reads the session for every websocket request (`ServeWebSocket`) and never
  touches the cache from `userUpdateActiveStatus`.
- **Now:** every frame gets `app.session(token)` (cache or database); a failed lookup answers the frame with the
  session error, as Go. Activity is recorded once at connect (`NewWebConn`). `touch_session` and
  `extend_session_if_needed` re-cache a session only when its row was updated (one row), so a revoked session never
  comes back. The reader stops before dispatching a frame once the writer is gone.
- **Checked:** a promoted user with an open socket demoted, then frames sent: REST `/config` 403. All sessions
  revoked, then frames: the socket closes, REST 401.

## SSO sign-up skipped the accepted domains (second review, fixed)

- **Was:** `create_user` checked `TeamSettings.RestrictCreationToDomains` only for email accounts, so a GitLab
  account with any email (gitlab.com, an open self-hosted GitLab) got a user despite the restriction; the guest
  magic link skipped the guests' list the same way. Go's `UserService.CreateUser` checks every user but LDAP and
  SAML ones, guests against `GuestAccountsSettings.RestrictCreationToDomains`.
- **Now:** `create_user_or_guest` sets the role first and checks as Go does; the magic link creates the guest
  through it (no `system_user` row in between).
- **Checked against Go** (third review): with `TeamSettings.RestrictCreationToDomains` = `allowed.org`, a GitLab
  sign-up and a GitLab login of a new `@offdomain.net` account (fake GitLab) are refused by both with 400 and the
  `api.user.create_user.accepted_domain.app_error` message on `/error`; no user is created. Go reaches the check
  only below its 250-user limit, so both ran on a scratch copy of the database with active users below it
  (dropped afterwards).

## Websocket events kept flowing on the connect-time session (third review, fixed)

- **Was:** a connection kept the token, `admin` flag and teams of its connect and no expiry; `publish` never
  checked the session again (only the `required_permissions` branch reloaded it). After `POST /oauth/deauthorize`
  or an OAuth refresh (neither closed the socket), after a demotion (admin-only events, unsanitised users with
  emails), or past the session's expiry, the socket kept streaming. Guests got `user_updated` and `new_user` about
  users they cannot see.
- **Now**, as Go's `WebConn` (`ShouldSendEvent`, `IsBasicAuthenticated`, `InvalidateCache`,
  `ShouldSendEventToGuest`):
  - a connection holds its session, with its admin and guest flags, teams and `ExpiresAt`;
  - `clear_sessions_for_user` (Go's `ClearSessionCacheForUser` → `Hub.InvalidateUser`) marks the user's
    connections stale; `POST /caches/invalidate` and license changes mark all of them (`InvalidateAllCache`);
    a revoked or rotated OAuth token marks its own (`invalidate_session`);
  - before an event goes to a stale connection or one past its `ExpiresAt` (which also covers the expired-session
    cleanup job), the session is loaded again (`app.session`) off the publisher's path; admin, guest and teams come
    from the fresh session, and required permissions are checked on it;
  - a session that cannot be loaded ends the deliveries for good (Go clears the token) while the socket stays
    open, its requests answered with the session error; the webapp then logs out at its next REST call, as with
    Go;
  - a guest session (`is_guest` = `true`) gets `user_updated` (`data.user.id`) and `new_user` (`data.user_id`)
    only when `can_see_user` says so; an error means no delivery. Only events that reach that check in Go (no
    connection, user, channel or team) are filtered;
  - the routing follows Go's order: admin-only data is checked for every event, also one sent to a user or
    connection; channel membership and the team only for events not sent to a user or connection (an
    `ephemeral_message` to a user in a channel it is not a member of now arrives, as with Go).
- **Cost:** a loaded, non-guest connection costs a few atomic loads per event; the database or cache is touched
  only for a stale or expired connection (once per invalidation) and for a guest on `user_updated`/`new_user`.
- **Deviations:** a server error while reloading (5xx) closes the socket so the client reconnects and catches up;
  Go silences the connection as for an invalid session. A session with `ExpiresAt` 0 is not reloaded for every
  event as Go does (Go fetches it from its cache each time); revocations reach it through the invalidation.
- **Checked against Go** (`ws99/wssec.py` in the session scratchpad, my Go on :8083 and Rust on :8099, same
  config copy with OAuth provider and guests on, `ShowEmailAddress` off), the events each observer socket got,
  identical on both:
  - OAuth: an `?access_token=` socket gets `user_updated` before `POST /oauth/deauthorize` and nothing after (the
    user's normal socket keeps getting it); after a refresh the old token's socket gets nothing, the new one's
    does;
  - demotion: before, `user_updated` with email and `job_updated` (admin-only); `user_role_updated`; after,
    `user_updated` without email and no `job_updated`;
  - expiry: `ExpiresAt` set in the past and `POST /caches/invalidate`: no `posted`; a session expiring 4 s after
    the connect with no invalidation: `posted` before, nothing after;
  - guest (a user session made a guest's: guests cannot log in without a license): `user_updated` of a user in a
    shared channel and of itself, none for a user in no shared channel. `new_user` was checked on Rust only (Go
    cannot create users past 250 on this database): none for the guest, delivered to a regular user.
  
  `diff.py --writes` 61/61, `routes.py` the 5 known diffs, `caches.py` 43/43; unit tests `hub::tests`.

## Websocket connections that stopped answering were never dropped (fixed)

- **Was:** the socket sent a ping every 60 s but had no read deadline: a client gone without a close (half-open
  TCP, a suspended phone) or one that never reads kept its connection, its send queue and the user's `online`
  status for as long as the kernel kept the socket. Many such clients hold memory and file descriptors without
  bound. Go's `readPump` sets a read deadline of `pongWaitTime` (100 s) that only a pong moves.
- **Now:** the same deadline: no pong for 100 s ends the connection, which then goes offline as a closed one does.
  A pong also runs `SetStatusAwayIfNeeded` (non-manual), as Go's pong handler: an idle user goes `away` after
  `UserStatusAwayTimeout`. Seen with `tools/diff/scen_live.py` (`ws_no_pong`).

## Import and export file names (2026-11-21)

`GET/DELETE /api/v4/exports/{name}` and `DELETE /api/v4/imports/{name}` joined the URL-decoded name to the export or
import directory. `files::clean` keeps the result inside the file store, but `..%2F..%2Fsomething.zip` left the
directory, so a system admin could download or delete any `.zip` in the store (attachments included). Go's route
variable never holds a `/`. Fixed: a name with `/`, `\` or `..` answers the router's 404.

## Guests: accepted domains and team counts (fixed)

- **Was:**
  - a guest could change its email (or an admin change it) to any domain: the update checked only
    `TeamSettings.RestrictCreationToDomains`, which guests are exempt from, never the guests' list
    (`GuestAccountsSettings.RestrictCreationToDomains`); Go refuses with `accepted_guest_domain` (`UpdateUser`).
  - joining a team checked a guest against the team's `allowed_domains` and the users' list, so a guest outside
    the guests' domains got in when its email matched the team's (and one inside them was refused). Go's
    `IsTeamEmailAllowed` checks a guest against the guests' list only.
  - `GET /teams/{id}/stats` gave a guest the team's full member count; Go counts only the members it can see.
  - after a promotion the user's cached sessions stayed guest sessions (roles and `is_guest`) until they expired.
- **Now:** as Go (`docs/USERS.md`, Guests). Checked with `tools/diff/scen_guests.py`.
