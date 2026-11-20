# Caches in front of the database

Mattermost answers the hot reads from memory. The Rust server has the same caches, with Go's sizes and lifetimes.
They are all in `src/cache.rs`, and the writes that make them stale drop the entries, as Go's do.

Two constraints follow from this.

- **One node.** Go sends cluster messages to invalidate caches on other nodes. We have one process, so an
  invalidation is a local `remove`.
- **One server at a time on a database.** Neither server sees the other's writes in its caches. Switch with a
  restart, never run both against the same database. That is how they take turns.

## Platform and app

| Go cache | Size, lifetime | What it serves | Rust |
| --- | --- | --- | --- |
| `Session` | 35000, `SessionCacheInMinutes` (10 in prod) | every request: token → session with team memberships | `caches.session`, `App::session` |
| `Status` | 35000, 30 min | `status/ids` (top endpoint), websocket status | `caches.status`, `status.rs` |
| `seenPendingPostIds` | 25000, 30 s | `POST /posts` retries with the same `pending_post_id` | `caches.seen_pending_post_ids` |
| `openGraphData` / link cache | 10000, 60 min | link previews in post metadata, keyed by URL and hour; then the `LinkMetadata` table (by its primary key) | `caches.open_graph`, `link_metadata_at`; permalink previews too, under their own keys |

## Store (`localcachelayer`)

| Go cache | Size, lifetime | Cached reads | Dropped by | Rust |
| --- | --- | --- | --- | --- |
| `UserProfileByIds` | 20000, 30 min | `User.Get`, `GetProfileByIds`, `GetMany` | any user update | `caches.user`, `App::users` |
| `AllUserProfiles` | 1, 30 min | `GetAllProfiles` page 0 of 100 without filters | any user update | `caches.all_profiles` |
| `ProfilesInChannel` | 25000, 15 min | `GetAllProfilesInChannel` | membership change, user update | `caches.profiles_in_channel` |
| `channelById` | 25000, 15 min | `Channel.Get`, `GetMany` | channel update/delete | `caches.channel`, `App::channel` |
| `ChannelByName` | 25000, 15 min | `GetByName(s)` | channel update/delete | `caches.channel_by_name` |
| `ChannelMemberCounts` | 25000, 30 min | `GetMemberCount` | membership change | `caches.member_count` |
| `ChannelGuestsCount` | 25000, 30 min | `GetGuestCount` | membership change | `caches.guest_count` |
| `ChannelPinnedPostsCounts` | 25000, 30 min | `GetPinnedPostCount` | pin/unpin, delete | `caches.pinned_count` |
| `ChannelMembersForUser` | 35000, 15 min | `GetAllChannelMembersForUser`: every channel permission check and websocket channel event | user's membership change | `caches.members_for_user`, `App::member_roles` |
| `ChannnelMembersNotifyProps` | 35000, 30 min | `GetAllChannelMembersNotifyPropsForChannel` (notifications) | notify props, membership | `caches.members_notify_props` |
| `LastPostTime` | 25000, 15 min | `GetPostsSince` answers "nothing new" without a query; post ETags | any post write in the channel | `caches.last_post_time` |
| `LastPost` | 20000, 30 min | `GetPosts` first page of 30 or 60 | any post write in the channel | `caches.last_posts` |
| `Team` | 20000, 30 min | `GetUserTeamIds` | team join/leave | `caches.team_ids_for_user` |
| `Role` | 20000, 30 min | `GetByName(s)`: every permission check | role save/delete | `caches.role`, `App::roles` |
| `Scheme` | 20000, 30 min | `Scheme.Get` | scheme save/delete | not needed: member roles come from the schemes joined in the same query, as Go's member queries do |
| `Reaction` | 20000, 30 min | `GetForPost` (post metadata) | reaction save/delete | `caches.reactions` |
| `FileInfo` | 25000, 30 min | `GetForPost`, `GetByIds` (post metadata) | attach, delete | `caches.file_infos` |
| `EmojiById`, `EmojiByName` | 5000, 30 min (not found: 60 s) | custom emoji in metadata and reactions | emoji save/delete | `caches.emoji_by_id`, `caches.emoji_by_name` |
| `TermsOfService` | 20000, 30 min | latest terms | save | `caches.terms_of_service` |
| `Webhook` | 25000, 15 min | incoming webhook by id (`POST /hooks/{id}`) | webhook update/delete | `caches.webhook`; Go drops the entry on every use (`UpdateIncomingLastUsed`), we update it in place, so it actually hits |
| `RolePermission` | 20000, 30 min | `ChannelHigherScopedPermissions`, run by `mergeChannelHigherScopedPermissions` on every role read (each permission check, `roles/names`) | role save | `caches.role_permissions`, merged in `App::roles` |
| The rest | | | | not reachable for EBAChat or without a license; see "Go's caches not ported" below |

## Beyond Go's caches

These are not caches, but they also take load off Postgres.

- **Threads queries.** Production Postgres scans about 17 billion `Threads` rows a day. The threads endpoints start
  from `ThreadMemberships` by `UserId`, which is indexed, and join `Threads` by its primary key. No index is added.
- **Websocket recipients.** For each channel event, membership comes from `ChannelMembersForUser`, as Go's
  `WebConn` does with its own copy.

## Verification

**Every cache in the tables above that the Rust server has is checked.** Each one must:

- hit when its read is repeated;
- drop the entry on the write that makes it stale;
- answer fresh after that write.

Each cache keeps Go's hit, miss and invalidation counters. System admins can read them at
`GET /api/v4/ebachat/caches`; Go exports the same counters as Prometheus metrics.

**Caches on EBAChat's paths, through the API (`tools/diff/caches.py`).** One server at a time on the database:

1. Do the cached read twice.
2. Do the write that must invalidate it.
3. Read again.

On Rust the counters are checked too; on Go only the answers.

| Run | Result |
| --- | --- |
| Rust `:8066` | 43/43 |
| Go `:8065`, the same steps (answers only) | 24/25 |

The checks cover:

- sessions (a revoked token is refused at once);
- statuses;
- profiles, and all profiles;
- channels by id and by name;
- member and pinned counts;
- memberships (joins and removals change access at once);
- member roles;
- notify props, and profiles in a channel;
- the last post time (`posts?since`);
- pending post ids (a retry gives the same post);
- reactions, file infos, custom emoji;
- link previews (permalink);
- roles;
- incoming webhooks (a deleted hook is refused at once).

**Caches Go reads only off EBAChat's paths (`tests/caches.rs`).** These run against the stand's database, calling
the same functions directly. All 4 pass.

| Go cache | Where Go reads it | What the test calls |
| --- | --- | --- |
| `LastPost` | `App.GetPosts`, used by AI recaps | `get_posts_cached`: the first page hits, a new post makes it fresh |
| `Team` (`GetUserTeamIds`) | `GetViewUsersRestrictions`, for users without `view_members` | `view_restrictions` and `can_see_user` of such a user: hit, then dropped on a team change |
| `Scheme` | `GetScheme`, `GetSchemeRolesForChannel`/`Team`: member role updates, the system console | `scheme`: hit |
| `TermsOfService` | the latest terms, for licensed custom terms of service | `latest_terms_of_service_id`: hit |

The Rust server reads them where Go does:

- **View restrictions** are ported as `GetViewUsersRestrictions`. They use the team ids and memberships caches.
  They are applied to user reads, lists and searches, and to `UserCanSeeOtherUser`.
- **Member role updates** read schemes through the scheme cache:
  - `PUT /channels/{channel_id}/members/{user_id}/roles`;
  - `PUT /teams/{team_id}/members/{user_id}/roles`;
  - `GET /schemes/{scheme_id}`.

## Go's caches not ported

These caches are the rest of `localcachelayer`. None of them is read on EBAChat's paths by a server without a
license:

- `docs/map/routes.tsv` gives the store calls each route can reach (`tools/mmmap`, an upper bound).
- Where a route can reach a cache's read, the Go code at the call shows what stands in front of it.

| Go cache | Reads | Can EBAChat's routes reach it? | What stands in front |
| --- | --- | --- | --- |
| `UserAutoTranslation`, `PostTranslationEtag` | `AutoTranslation.GetUserLanguage`, `IsUserEnabled`, `GetLatestPostUpdateAtForChannel` | no | |
| `DeliveryTracking` | `DeliveryTracking.*` | no | |
| `PostsUsage` | `Post.AnalyticsPostCount` | no | cloud usage limits |
| `FileInfo` storage usage | `FileInfo.GetStorageUsage` | no (only the admin `analytics/old`) | |
| `AccessControlPolicyEtag` | `AccessControlPolicy.GetEtagEpoch` | yes, in `AppendABACEtag` | `attributeBasedAccessControlEnabled()`: the `PermissionPolicies` flag and `EnableAttributeBasedAccessControl` (off in production), an Enterprise Advanced feature |
| `UserAttributes`, `UserPropertyValuesEpoch` | `Attributes.GetSubject`, `GetUserPropertyValuesEpoch` | yes, in `AppendABACEtag` and the access control code | the same ABAC switch |
| `SessionAttribute` | `SessionAttribute.Get`, `GetEpoch` | yes, in `AppendABACEtag` and session attributes | ABAC, and `sessionAttributesEnabled()`: a feature flag and an Enterprise Advanced license |
| `PropertyField` | `PropertyField.GetForGroup` | yes, in session attributes | `sessionAttributesEnabled()` |
| `ContentFlagging` | `ContentFlagging.GetReviewerSettings` | yes, in content flagging | an Enterprise Advanced license; `EnableContentFlagging` is off in production |
| `ReadReceipt`, `ReadReceiptPostReaders`, `ReadReceiptPostUnreadCount`, `TemporaryPost` | `ReadReceipt.*`, `TemporaryPost.Get` | yes, for burn-on-read posts | Only burn-on-read posts read them. `FillInPostProps` refuses such posts without an Enterprise Advanced license (501), even with `EnableBurnOnRead`, which production has on. The stand's database has none. |

Every one of them is a licensed feature or not on EBAChat's paths at all. The Rust server answers these features as
an unlicensed Go server does, so there is nothing for these caches to serve.

**`RolePermission` was missing from the Rust server, and is ported now.**

- `mergeChannelHigherScopedPermissions` runs on every role read, and so on every permission check: it limits a
  channel scheme's moderated permissions to what the higher scope allows.
- Checked on the stand with a channel scheme whose user role lacks `create_post`. Go and Rust agree on:
  - the member's roles;
  - the merged permissions of `roles/names` (27 in the role, 26 after merging);
  - the refused post.
- The cache hits in `tools/diff/caches.py`.

**Go's one failure is a bug in Go, not in the check.**

- Go caches a DM by name under the key `"" + name`, but `InvalidateCacheForChannel` drops `"dm" + name`.
- So after a DM's header changes, `POST /channels/direct` answers with the old channel for up to 15 minutes.
- The Rust server caches DMs by name the same way, and the repeated read hits, but it drops both keys. This is the
  one place where its caching deliberately differs from Go's, and only in being fresh.

**Rules to know:**

- **Every new post drops its channel from both channel caches**, as `handlePostEvents` does. The by-name cache
  serves the reads between posts: direct channels, webhooks, slash commands, team joins.
- **One server at a time.** Go's caches cannot see the Rust server's writes, and the Rust server's caches cannot
  see Go's. The servers take turns with a restart.
