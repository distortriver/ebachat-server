# Post search

Database post search (`POST /teams/{id}/posts/search`, `POST /posts/search`), the way Go's SQL store does it
(`SearchPostsForUser`: full-text `to_tsvector`/`to_tsquery`, the term syntax of `search_params.rs`, channels the user
is a member of, permission filter after the query), with one deliberate difference: paging.

## Paging (unlike Go)

Go's database search answers at most the newest 100 matches on page 0, whatever `per_page` asks, and an empty list
for any later page, so older matches cannot be reached. Rust pages for real:

- `per_page`: default 60 as in Go; `<= 0` means 60; at most 1000.
- `page`: from 0 (negative means 0); posts `[page * per_page, page * per_page + per_page)` of the matches, newest
  first (ties by id, so pages never overlap or skip).
- Depth: the newest 10 000 matches per set of terms; a page past that is empty.
- The answer keeps Go's shape (`order`, `posts`, `matches` when present; no `has_next`).

The clients page unchanged: the web app (`WEBAPP_SEARCH_PER_PAGE` = 20) asks `page + 1` while a page is not empty,
EBAChat while a page is as long as its `per_page`. With Go they stop after the first page; with Rust they go on to
older matches.
