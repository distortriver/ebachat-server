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
  - no `Expires: 0`, whatever the status.
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

## Checks

- **Scripts:** the flows were run against a Go server and the Rust server on one database:
  - the OAuth provider: register an app, then the code, PKCE, implicit and refresh grants, a resource mismatch,
    about 15 error cases, and deauthorize;
  - GitLab SSO, against a fake GitLab both servers use;
  - the unsupported pages, for several user agents;
  - Range and conditional requests on a static file.
- **Result:** answers, redirects, cookies, session props and rows are the same, apart from ids, times, signatures
  and the differences above.
