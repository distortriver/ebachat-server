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
- **Checks:** `tools/diff/routes.py --area properties`, and every group (real, unknown, badly named) with every object
  type, as admin and as a member, for every route: same status and body as Go, writes included.

## Not ported

- **Option hierarchies of `graph` fields:** an option's `parents` are read (the options listing), but a write that
  sets parents (an inline option list or the options routes) answers 501 `api.not_implemented.app_error`. Nothing
  uses graph fields yet.
- **Audit records:** Go writes property audit events to its audit log; the Rust server writes none.
- **Hooks of licensed groups** (access control masking, attribute validation, the value audit, the session
  attributes guard): their groups are refused before any of them would run.
