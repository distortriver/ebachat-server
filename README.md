# ebachat-server

A chat server written in Rust, the server half of EBAChat. For now it runs on an existing Mattermost database
(PostgreSQL schema of Mattermost 12) and speaks the Mattermost REST API v4 and websocket protocol, so the same
database can be switched between the Go server and this one.

## License

AGPL-3.0-only, see [LICENSE](LICENSE). The server is written with the Mattermost server's source as a reference,
which is licensed to us under AGPL-3.0; the result is therefore distributed under the same license.

Code from Mattermost's `server/enterprise/` (Mattermost Source Available License) is never read, ported or copied
here.

Anyone who uses a running instance over the network must be offered this program's source (AGPL-3.0, section 13).

Mattermost is a trademark of Mattermost, Inc. This project is not affiliated with or endorsed by Mattermost, Inc.
