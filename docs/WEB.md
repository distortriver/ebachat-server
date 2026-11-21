# Web routes: where the Rust server differs from Go

The routes outside `/api/v4` that Go's `channels/web` registers: the web client and its static files, the pages Go
shows instead of the client, the OAuth 2.0 provider, SSO sign-in and a few redirects. They are served by
`src/web.rs`, `src/unsupported.rs`, `src/oauth_provider.rs` and `src/sso.rs`, as an unlicensed Go 12.1 server
answers them.

- **Errors:** Go's web error page, either an HTML page or a 3xx redirect to the client's signed
  `/error?message=...&s=...` (`RenderWebAppError`). The answer is JSON only with `X-Mobile-App`.
- **Router 404:** a path outside `/api/` that no route takes:
  - GET and HEAD get the client's page;
  - any other method gets Go's plain-text `404 page not found`, not the API's JSON 404.

## Web client

- **Static files:** `/static/*` from the client directory (`EBACHAT_CLIENT_DIR`, default `client/`) and
  `/static/plugins/*` from `PluginSettings.ClientDirectory`, as Go's file server sends them:
  - every method is answered as GET;
  - `Last-Modified`, the preconditions of RFC 7232 (`If-Match`, `If-None-Match`, `If-Modified-Since`,
    `If-Unmodified-Since`, `If-Range`, no ETag), and byte ranges: one range is a 206 with `Content-Range`, several
    are `multipart/byteranges`, and a range past the end is a 416;
  - compressed as Go's `gzhttp` (klauspost/compress 1.19) does it: zstd or gzip by the `Accept-Encoding`
    q-values (zstd when equal), only a full 200 of at least 1 KiB of a type its default filter takes, and `Vary:
    Accept-Encoding` on every answer;
  - no `Expires: 0`, whatever the status, and only `setStaticSecurityHeaders`' headers (no `X-Request-Id` or
    `X-Version-Id`);
  - `/static/` and `/static/plugins/` themselves are the file server's 404;
  - compressed only with `WebserverMode` "gzip"; none of these routes with "disabled".
- **Offline docs:** `/_documentation/*` from `<client>/_documentation` as Go's docs handler serves it (clean URLs
  tried as `<path>.html`, then `<path>/index.html`; HTML pages `no-cache`; Go's 404 otherwise), and
  `/_documentation` redirected (302) to `/_documentation/`.
- **The client's page:** every other GET or HEAD outside `/api/` gets `root.html`, with the site name and
  description. It is byte-identical to Go's.
- **Go's main router** serves `/robots.txt` (any method) and `/unsupported_browser.js` (any method) without the
  headers `web.Handler` adds.
- **Other routes:**
  - `GET|HEAD /files/{id}/public`;
  - `GET|POST /login/sso/saml`: always "SAML not available" (302);
  - `/manualtest`: the client's page unless `EnableTesting` is on.

## Unsupported browsers and desktop apps

- **Which page:** instead of the client's page, Internet Explorer and Safari before 12 get `unsupported_browser`. A
  desktop app older than `ServiceSettings.MinimumDesktopAppVersion` gets `unsupported_desktop_app`.
- **Templates:** both are Go's own templates, copied into `templates/` (Apache-2.0, see `NOTICE`). They are rendered
  as Go's `html/template` renders them.
- **Detection:** the browser detection follows `uasurfer` for the two browsers Go checks.

## OAuth 2.0 provider

- **The authorize page:** `GET /oauth/authorize` with its redirects and web error pages. A visitor who is not
  logged in goes to `/login?redirect_to=...`, or to the SAML login with `login_hint=saml`.
- **The API:** `POST /oauth/authorize`, `POST /oauth/access_token` (authorization code with PKCE S256, refresh
  token, RFC 8707 `resource`), `POST /oauth/deauthorize` and `GET /.well-known/oauth-authorization-server[/...]`.
- **Rows:** codes, access tokens, OAuth sessions and the authorized-app preference are written as Go writes them,
  including the limit of 500 sessions.
- **Encoding:** answers use Go's JSON encoding: HTML-escaped, with a trailing newline.
- **Not served:** `/api/v4/oauth_test`. Go registers it, but its API router answers 404 first.

## SSO sign-in

- **GitLab is the only provider,** as on a server without a license. Google, Office 365 and OpenID answer Go's "not
  available" or "unsupported" errors.
- **Routes:**
  - `/oauth/{service}/login`, `mobile_login` and `signup`;
  - the callback at `/oauth|signup|login/{service}/complete` and `/api/v3/oauth/{service}/complete`.
- **The callback:** the `MMOAUTH` cookie and the state token, the code exchange, then creating, logging in or
  updating the user. It also handles invite ids and tokens, reactivation, the desktop-token redirect and both mobile
  variants.
- **The desktop app:** `POST /api/v4/users/login/desktop_token` logs the SSO user in.
- **Switching accounts:** switching from email to GitLab answers the `follow_link` and sets the cookie.
- **Other routes:**
  - `/oauth/intune`: 501, not available without a license;
  - `/login/one_time_link`: the guest magic link.

## Differences

- **Same email, different SSO account:** never re-linked. GitLab's `IsSameUser` compares two pointers in Go, so the
  outcome is the same.
- **The desktop-token login answer** leaves out the user's password hash and MFA secret. Go writes them back.
- **Compressed bytes:** a gzip or zstd body decompresses to the same bytes, but the compressed bytes differ from
  Go's (other implementations).
- **Multipart boundaries** are random, in Go as well.
- **API answers** are compressed as Go's (`src/compress.rs`, see `docs/MAP.md`); the `Audits` rows of these
  routes are written as Go's (`docs/ADMIN.md`).

## Sessions on web routes

A failed CSRF check, a query-string token of a non-OAuth session or a 500 from the session lookup stops the request
before the handler, as Go's `ServeHTTP`: JSON under `/api/`, for webhooks and the OAuth API, the web error page
elsewhere (`app::MaybeAuth`, `app::error_response`).

## Checks

- **`WebserverMode`:** 13 paths (the client's page, static files, `/static/`, robots, the unsupported-browser
  script, the docs, plugins, the API) by GET and POST, against my Go with "gzip", "uncompressed" and "disabled":
  the same status, encoding, `Vary` and type, 26/26 each.

- **Scripts:** the flows were run against a Go server and the Rust server on one database:
  - the OAuth provider: register an app, then the code, PKCE, implicit and refresh grants, a resource mismatch,
    about 15 error cases, and deauthorize;
  - GitLab SSO, against a fake GitLab both servers use;
  - the unsupported pages, for several user agents;
  - Range and conditional requests on a static file.
- **Result:** answers, redirects, cookies, session props and rows are the same, apart from ids, times, signatures
  and the differences above.
- **`tools/diff/scen_sso.py`** (sqldiff, a fake GitLab per server): email to GitLab and back through
  `/users/login/switch`, web, mobile (with and without `redirect_to`) and repeated logins, attributes GitLab changed
  (rename, name, taken username/email), bad states (garbage, unknown token, other action, no cookie), a missing code,
  `access_denied`, a failed code exchange (web and mobile), a GitLab user without email, sign-ups refused by
  `RestrictCreationToDomains` or an email already taken, new accounts (sign-up, login, mobile, a taken username), GitLab
  disabled. Same statuses, redirects, cookies, rows, audits and events, except Go's 250-user cap. It found:
  - `UpdateOAuthUserAttrs` goes through the store's `Update`, which marks an SSO account's email verified and
    rewrites the mention keys of a renamed user; Rust wrote only the changed columns (now `store_update_user`, the
    store part of `update_user`, which also keeps an OAuth account's email on an untrusted update as Go's store does);
  - a user created verified (SSO, invitation token, magic link) gets `VerifyEmail` once more and a `user_updated`
    to everyone (`createUserOrGuest`); every new user is marked as having seen the last fetched product notices
    (`UpdateViewedProductNoticesForNewUser`), so Go does not show them those notices later;
  - only the API's `createUser` (`SanitizeInput`) stores an empty `RemoteId`; SSO, magic-link and imported users
    keep NULL (no `remote_id` in their JSON), as in Go.

## CORS and method fallthrough

- **CORS** (`src/cors.rs`): when `ServiceSettings.AllowCorsFrom` is set, the whole router is wrapped as Go wraps
  its handler in `rs/cors` 1.11.1 with Mattermost's options (`corsAllowedMethods`, all headers allowed, max age
  86400, `CorsExposedHeaders`, `CorsAllowCredentials`): preflights get 204 with the allow headers for an allowed
  origin and method, every other answer gets `Vary: Origin` first and the allow headers for an allowed origin.
  Built once at start, as Go's wrapper. Checked against Go with the same setting: preflight allowed and refused,
  actual request with an allowed, a foreign and no origin, an unlisted method.
- **A static path that does not take the method** (`GET /users/search`): gorilla goes on to the next routes, so
  the parameter route at that place answers (`getUser` with id "search": 400 `invalid_url_param`); axum stops at
  the static route. `api::method_mismatch` routes such a request again with one segment percent-encoded (which
  only a parameter can match, and which decodes back), from the last segment to the first, else the 404. Checked
  on 15 such paths against Go.
