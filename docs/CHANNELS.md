# Channels and the sidebar: where the Rust server differs from Go

- **Code:** `src/channels.rs` (the app layer), `src/channels_api.rs` and `src/channel_admin.rs` (the routes),
  `src/sidebar.rs` (sidebar categories).
- **Checks:** `SQLDIFF_SCENARIO=channels ./dev/bench/sqldiff.sh chan 8088` (`tools/diff/scen_channels.py`): create,
  patch and update, members (paging, ids, stats, many at once, remove, leaving a private channel as its last
  member), notify props, scheme roles, privacy, moving to another team, archiving with and without webhooks,
  restoring, the archived and private lists, search and autocomplete, and the sidebar categories (create, rename,
  sorting, mute, collapse, order, moving channels, favorites through the category and the preference, delete, where
  new channels, DMs and GMs land). Answers are compared by value (names, counts, flags), not only by keys.

## Go bugs fixed, not reproduced

- **Channel member notify props are validated.** Go's `UpdateChannelMemberNotifyProps` merges the given props
  into the row unchecked, so `"mark_unread": "sometimes"` is stored (200), and from then on every `UpdateMember` of
  that member (roles, scheme roles) fails `ChannelMember.IsValid` with 400. Rust refuses the update with the error
  `IsValid` would give (`model.channel_member.is_valid.*.app_error`, 400) for `desktop`, `mark_unread`, `push`,
  `email`, `ignore_channel_mentions` and `channel_auto_follow_threads`.
- **`PUT /channels/{id}` without a display name** keeps the display name, as Go does, but Go still posts
  "updated the channel display name from: X to: " (its check compares the old name with the body's empty one).
  Rust posts only a real change.
