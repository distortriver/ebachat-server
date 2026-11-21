# Branding: the site's name instead of Mattermost

A deliberate difference from Go. Go's texts name "Mattermost". This server shows the site's name instead:
`TeamSettings.SiteName`, or **"EBAChat"** when that setting is empty or still Go's default "Mattermost".
Only the **texts** change. Error ids, translation ids, API paths, JSON keys, config keys, cookie and header names
(`MMAUTHTOKEN`, `X-Version-Id`, ...) are the same as Go's, so clients see no other difference.

The name comes from the config in memory: `crate::brand` sets it at start and on every config save or reload
(`Config::install`), and readers read one pointer.

## What is replaced

- **Translations** (`i18n/*.json`, every locale), at render time in `src/i18n.rs` (`t`, `tr`, `tr_n`, `tl`):
  API error messages, system posts, slash command replies, ephemeral messages, push and email texts, websocket
  payloads. The locale files are not edited. `src/brand.rs` rewrites a text before its parameters are filled
  in, so a user's own words are never touched:
  - the name, also inflected ("Mattermostu" in Czech becomes "EBAChatu") and in Korean ("매터모스트");
  - Mattermost's other products get neutral wording: "Mattermost Desktop App" is "the desktop app", "the
    Mattermost app" is "the app", "Mattermost Enterprise Edition" is "Enterprise Edition", "Mattermost Cloud"
    is "Cloud", "Mattermost license" is "license", "Mattermost v12.0" is "v12.0" (the list is `PHRASES`);
  - identifiers (`{{.MattermostUsername}}`, `MattermostAppID`) and lowercase URLs and addresses
    (`docs.mattermost.com`) stay.
- **Messages not from the translations:** an `AppError`'s fallback text, `data/unlicensed_routes.txt`'s
  messages, the built-in slash commands' descriptions and hints (`data/builtin_commands.json`).
- **Emails:** `SiteName` in subjects and bodies is the name above. The footer line "© 2015 - year Mattermost,
  Inc. 2100 Geng Road, ... Palo Alto" (`api.templates.email_footer_v2`) is dropped. The email change
  verification gives `SupportSettings.SupportEmail` (Go: `feedback@mattermost.com`) and shows the line only when
  it is set. The invite's "learn more" links to the site (Go: `mattermost.com`). These two template changes are
  in `data/email_templates.txt`: regenerating it with `tools/emailtpl` must keep them.
- **Pages:** the client page's `<title>` (Go: "Mattermost" when SiteName is empty); the unsupported desktop app
  page's footer ("© year <site>", Go: "Mattermost Inc."); the unsupported browser page's download link is
  `NativeAppSettings.AppDownloadLink` (Go: Mattermost's download page).
- **Client config:** `SiteName` in `/api/v4/config/client` is the name above, which is what the clients show.
  The config itself (`/api/v4/config`, the System Console) keeps what the admin set.
- **MFA:** the issuer of the `otpauth://` link when `SiteURL` is empty.

## What is kept

- Ids, keys, paths, cookies and headers, as above; `MattermostAppID` and other column and field names.
- User agents: the desktop and mobile apps' `Mattermost/` and `Mattermost Mobile/` that `src/user_agent.rs` and
  `src/unsupported.rs` parse; the link preview fetcher's `Mattermost-Bot/1.1` (sites recognise link-preview
  bots by it); the outgoing mail's `go-mail` `User-Agent` and `X-Mailer` (they do not name the product).
- Links to Mattermost's documentation in texts (`docs.mattermost.com`, `mattermost.com/pl/...`).
- Logs, file names (`mattermost.log`), export formats (`"generator": "mattermost-server"`), feature names.
- Code comments, license headers, `NOTICE`, the attribution AGPL-3.0 and Apache-2.0 require.
