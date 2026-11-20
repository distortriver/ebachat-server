# Properties: where the Rust server differs from Go

The property routes under `/api/v4/properties/groups/{group_name}/` (fields, field options, values, system values,
field search) are served by `src/properties.rs`, as an unlicensed Go 12.1 server answers them. Go registers them
because the `ClassificationMarkings`, `SessionAttributes` and `ChannelAttributes` feature flags are on by default.

- **Groups:** the `PropertyGroups` rows. Only version-2 groups are served:
  - `boards` and `post_attributes` work without a license;
  - `access_control` needs an Enterprise license for any field, option or value it holds: reads that find nothing
    answer `[]` (fields) or `null` (values), anything else is `app.property.license_error` (403). Its `channel`
    objects need Enterprise Advanced (501);
  - `session_attributes` needs Enterprise Advanced (501);
  - `content_flagging` (version 1) and `managed_channel_categories` (version 3) are `v2_group_not_found` (404), an
    unknown group `app.property_group.get.app_error` (404).
- **Route patterns:** a group name, object type or id that fails Go's route pattern is the router's 404.
- **Options:** an option-bearing field carries its `PropertyOptions` rows inline in `attrs.options`, or
  `options_count` and `options_omitted` past 1,000.
- **Websocket:** `property_field_created`, `property_field_updated`, `property_field_deleted` and
  `property_values_updated`, with Go's broadcast (team, channel or everyone) and withheld option lists and values
  for fields that are not public.
- **Option hierarchies of `graph` fields** (`src/properties/graph.rs`): an option names the options right above it
  in `parents`.
  - **Where parents are set:** in a field's inline option list (create and update) and on the options routes
    (create and patch).
  - **How names resolve:** an inline list resolves names within itself. The options routes resolve against the
    field's options plus the payload's own new names.
  - **Replacing parents:** an option that states its `parents` gets exactly those. One that leaves the key out keeps
    its parents.
  - **Checks on a change:** a change is checked as a whole, against the hierarchy it leaves. A rejection is
    `app.property_field.options.invalid.app_error` with Go's reason.
    - at most 100 parents per option;
    - at most 1,000,000 links per field;
    - no cycle;
    - no chain longer than 100 options.
  - **Writes:** the links are written in the same transaction as the options, under the field's `UpdateAt`.
  - **Unlinking:** a field unlinked from its template takes over the template's options and links.
  - **Deleting:** an option with children cannot be deleted alone, by name or by leaving it out of a field's list.
- **Checks:** `tools/diff/routes.py --area properties`, and every group (real, unknown, badly named) with every object
  type, as admin and as a member, for every route: same status and body as Go, writes included. For graph
  hierarchies, a script ran the same flow against Go and Rust: inline lists, the options routes, renames, cycles,
  depth, fan-in, deletes and field writes. The answers were the same.

## Not ported

- **Audit records:** Go writes property audit events to its audit log; the Rust server writes none.
- **Event order:** when a type change clears a channel-scoped field's values, Go sends `property_field_updated`
  before `property_values_updated`; the Rust hub may deliver the channel broadcast after the global one.
- **Hooks of licensed groups** (access control masking, attribute validation, the value audit, the session
  attributes guard): their groups are refused before any of them would run.
