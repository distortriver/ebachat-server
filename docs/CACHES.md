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
| `openGraphData` | 10000, 60 min | link previews in post metadata | `caches.open_graph`, `link_metadata` |

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
| `Scheme` | 20000, 30 min | `Scheme.Get` | scheme save/delete | `caches.scheme` |
| `Reaction` | 20000, 30 min | `GetForPost` (post metadata) | reaction save/delete | `caches.reactions` |
| `FileInfo` | 25000, 30 min | `GetForPost`, `GetByIds` (post metadata) | attach, delete | `caches.file_infos` |
| `EmojiById`, `EmojiByName` | 5000, 30 min (not found: 60 s) | custom emoji in metadata and reactions | emoji save/delete | `caches.emoji_by_id`, `caches.emoji_by_name` |
| `TermsOfService` | 20000, 30 min | latest terms | save | `caches.terms_of_service` |
| `Webhook` | 25000, 15 min | incoming webhook by id | webhook update/delete | not yet (incoming webhooks are T2) |
| `RolePermission` | 20000, 30 min | `ChannelHigherScopedPermissions` (admin console) | role save | not needed by T1 |
| `PostsUsage`, `ReadReceipt*`, `TemporaryPost`, `ContentFlagging`, `DeliveryTracking`, `PropertyField`, `UserAttributes`, `AccessControlPolicyEtag`, `UserAutoTranslation`, `PostTranslationEtag`, `SessionAttribute` | | licensed, cloud or plugin features | | not ported |

## Beyond Go's caches

These are not caches, but they also take load off Postgres.

- **Threads queries.** Production Postgres scans about 17 billion `Threads` rows a day. The threads endpoints start
  from `ThreadMemberships` by `UserId`, which is indexed, and join `Threads` by its primary key. No index is added.
- **Websocket recipients.** For each channel event, membership comes from `ChannelMembersForUser`, as Go's
  `WebConn` does with its own copy.
