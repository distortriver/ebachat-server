# Notifications: push and email

The Rust server sends mobile pushes and emails as an unlicensed Go 12.1 server does: the same recipients, the same
push proxy payloads, the same email headers and bodies.

## Files

| File | What | Go |
| --- | --- | --- |
| `src/notify.rs` | Who gets what for a new post: mentions, followers of collapsed threads, "all activity" pushes, `userAllowsEmail`, the "too many members for @channel" ephemeral posts | `app/notification.go` |
| `src/push.rs` | Push queue and workers, message building, the push proxy client, clear and badge pushes, acks, test pushes, session wipe pushes | `app/notification_push.go`, `model/push_notification.go` |
| `src/email.rs` | Every email Go sends (`Send*Email`), notification emails and their batching, invitation rate limits, verification and recovery tokens | `app/email/*.go`, `app/notification_email.go`, `utils/time.go` |
| `src/mail.rs` | One email over SMTP (`net/smtp` session, STARTTLS/TLS, PLAIN/LOGIN) and the MIME message as `go-mail` writes it; the text part by `html2text` | `platform/shared/mail/mail.go` |
| `src/email_template.rs`, `data/email_templates.txt` | The HTML templates, rendered as `html/template` renders them | `templates/*.html` |
| `src/markdown.rs` | A message as plain text for push (`StripMarkdownAndDecode`) and as HTML for email (`MarkdownToHTML`, GFM) | `utils/markdown.go` |
| `src/i18n.rs` | Strings in the user's locale, with plurals (`i18n/*.json`, loaded on first use from `EBACHAT_I18N_DIR`, default `i18n/`) | `public/shared/i18n` |

### Templates

`html/template` escapes by context (text, attribute, URL, CSS) and drops HTML comments. Rather than reimplement
that, `tools/emailtpl` (Go) loads `server/templates/*.html` exactly as the server does, lets `html/template` escape
them, and dumps the escaped trees: every action already names its escapers
(`{{.Props.ButtonURL | _html_template_urlfilter | _html_template_urlnormalizer | _html_template_attrescaper}}`).
`src/email_template.rs` only evaluates those pipelines (`.Field`, `$`, `if`, `range`, `template`, `len`, `eq`, `or`)
and runs the escapers. Regenerate with:

```sh
cd tools/emailtpl && go run . -templates ../../../ebac-messenger/mattermost/server/templates \
  -only $(cat ../../data/email_templates.list) > ../../data/email_templates.txt
```

## Push

- **When:** `EmailSettings.SendPushNotifications`, and not a Mattermost-hosted push server (those need a license).
- **Who gets a message push** (`SendNotifications`):
  - the mentioned (explicit, DM, GM, thread and comment mentions), by `ShouldSendPushNotification`;
  - members whose user or channel push setting is "all";
  - followers of a collapsed thread (`crt` type), by their thread settings;
  - the user's status, `push_status` and active channel decide, as Go.
- **Payload:** `model.PushNotification` field for field, `omitempty` as Go. `PushNotificationContents` `full`,
  `generic`, `generic_no_channel`; `id_loaded` needs the licensed notification service, so it is `generic`, as in Go.
  The badge is the unread mention count (root mentions plus thread mentions with collapsed threads).
- **Per device:** every live session with a device id (`GetSessionsWithActiveDeviceIds`) gets its own copy, with a
  new `ack_id` and an ES256 JWT `signature` from the server's `AsymmetricSigningKey`. VoIP tokens are used for
  `voip` transport pushes only, as Go.
- **Proxy:** `POST <PushNotificationServer>/api/v1/send_push`, 30 s timeout. `REMOVE` marks the session's
  `last_removed_device_id` (or `last_removed_voip_device_id`) and drops its cache; the device gets nothing more.
  No auth token headers: Go sets them only with the enterprise push proxy service.
- **Queue:** a bounded queue of `PushNotificationBuffer` and `NumCPU * 8` workers, as `PushNotificationsHub`.
- **Clear pushes:** viewing channels (`channels/members/{id}/view`, `read`, a post by the user) clears the channels
  with unreads per the push setting (`GetChannelsWithUnreadsAndWithMentions`) on the user's other devices; reading
  a thread to the end with collapsed threads clears it. **Badge pushes:** marking a post unread.
- **Acks** (`POST /notifications/ack`): passed on to `<server>/api/v1/ack`; an iOS ack turns
  `device_notification_disabled` off again. An id-loaded ack for a post answers 302
  `api.system.id_loaded.not_available.app_error` after the read check, as Go without the licensed service.
- **Test push:** `GET /system/ping?device_id=` answers `CanReceiveNotifications` `true`, `false` or `unknown`.
- **Session wipe** (`sendMobileWipeSignal`) is in `push::wipe`, for `MobileEphemeralModeSettings.Enable`.

## Email

- **SMTP:** a session per email, as Go: `EHLO <SiteURL host>`, STARTTLS when asked and offered, `TLS` dials TLS,
  PLAIN or LOGIN auth (both need TLS; PLAIN also localhost), `MAIL FROM` with `BODY=8BITMIME`/`SMTPUTF8` when
  offered, no `QUIT` (Go closes first). Certificates are checked against the platform's roots unless
  `SkipServerCertificateVerification`.
- **Message:** go-mail's headers in its order (generic headers sorted, then From, To, Cc, Reply-To),
  `Auto-Submitted`, `Precedence: bulk`, `Message-ID` (`<postid@host>` for notifications, with `In-Reply-To` and
  `References` for replies), subject Q-encoded when not ASCII, `multipart/alternative` with the text and HTML parts
  in quoted-printable, `multipart/related` with inline images (`user-avatar.png`). The SendGrid category header
  needs a cloud license and is never set.
- **Notification emails:** to the mentioned and to followers who want them, when `SendEmailNotifications`, the
  user's and channel's email settings allow it, the user is not online or DND, and (with `RequireEmailVerification`)
  verified. Subject, title and time in the user's locale and time zone, message as GFM HTML with `~channel` links,
  attachments, the sender's picture. `EmailNotificationContentsType` needs a license: always full.
- **Batching** (`EnableEmailBatching`): users whose `email_interval` is not "immediate" (`30`) are batched; every
  `EmailBatchingInterval` the batches older than the user's interval (default 900 s) go out as one email, unless
  the user read the team's channels since. (Checked by reading the Go code; not run against Go, whose shortest
  batch is 15 minutes unless the preference is edited.)
- **Transactional emails**, wired where Go sends them:

  | Email | Route |
  | --- | --- |
  | Welcome (with a verify link when verification is required) | `POST /users` (invite id, token, admin, open sign-up) |
  | Verify email / verify new email | `POST /users/email/verify/send`, `PUT /users/{id}/patch` with a new email and verification required |
  | Email changed (to the old address) | patch with a new email, `POST /users/email/verify` for a change |
  | Username changed | patch with a new username |
  | Password reset link | `POST /users/password/reset/send` |
  | Password changed | `PUT /users/{id}/password` (own, admin, hashed), `POST /users/password/reset` |
  | MFA activated/deactivated | `PUT /users/{id}/mfa` |
  | Account deactivated | `PUT /users/{id}/active` by the user |
  | Personal access token added / rotated | `POST /users/{id}/tokens`, `POST /users/tokens/rotate` |
  | Team invitations (with channels and message) | `POST /teams/{id}/invite/email` (and `?graceful`), `/invite_people` |
  | Test email | `POST /email/test` |

- **Invitations:** a `team_invitation` token per address, the signup link as Go builds it, 20 per hour per sender
  (GCRA with a burst of 20, as `throttled`), 413 `app.email.rate_limit_exceeded.app_error` beyond. Graceful
  invitations also save the `resend_invitation_email` job Go saves. Signing up with such a token
  (`POST /users?t=`) is ported (`CreateUserWithToken`): the email must match, the user joins the team and the
  channels the inviter may still add to, the token is deleted.
- **Not reachable without a license:** guest invitations and magic links (`SendGuestInviteEmails`,
  `SendMagicLinkEmailSelfService` are ported in `email.rs` for when they are), license, cloud and IP filter
  emails. `email::send_sign_in_change` (`SendSignInChangeEmail`) is there for the SSO sign-in switch routes.

## Verification against Go

A local SMTP sink and a fake push proxy (logging every message; device ids containing `remove` answer `REMOVE`,
`fail` answer `FAIL`), one pair for a Go 12.1 instance and one for Rust, the same config otherwise. The same
scenarios ran against both, and the records were compared after normalizing ids, random tokens and times:

- **Pushes:** mention, `@channel`, DM, GM, thread reply (collapsed-thread follower), attachment fallback, channel
  view (clear), thread read (clear), mark unread (badge), reaction (nothing, as Go), acks (plain, id-loaded,
  empty body), test pushes (`true`, `false` on `REMOVE`, `unknown` on `FAIL`), `REMOVE` marking the session.
  Payloads identical.
- **Emails:** the notification emails of those scenarios (96 of them, `@channel` reaches every member who is
  offline) and the 13 transactional emails above: identical headers that matter, MIME structure, text part and
  HTML part.
- **Errors** of `POST /email/test` and the routes above: the same status and id.

## Deviations from Go

- **`POST /email/test` sends over the settings in the request.** Go checks those but sends over the saved
  settings, so the System Console's "Test Connection" reports success for unsaved, broken settings.
- **Clear and badge pushes only when pushes can be sent.** Go queues them even with `SendPushNotifications` off
  and posts them to whatever server is configured.
- **A blockquote at the very start of a message is a blockquote in email.** Go's regex (`^|\n(&gt;)`) only
  unescapes `>` after a newline, so a message starting with `>` shows a literal `&gt;`.
- **DM notification emails keep the recipient's own team.** Go's `sendNotificationEmail` switches to
  `select_team` when the recipient *is* in the channel's team (an inverted check); for DMs the team is empty, so
  both pick the user's first team.
- **Badge counts** sum with `COALESCE`: Go fails the push when the user is in no channel (`SUM` of nothing is NULL).
- Server log lines are not Go's (`mlog` notification levels and metrics are not ported).
