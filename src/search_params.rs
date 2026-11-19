//! A search box's text as Mattermost reads it: terms, excluded terms, hashtags, and the `in:`, `from:`, `before:`,
//! `after:`, `on:` and `ext:` flags (with `-` to exclude). Posts and files search with it.

// Based on Mattermost server/public/model/search_params.go and model/utils.go (date helpers), and
// store/sqlstore/utils.go (removeNonAlphaNumericUnquotedTerms), Copyright (c) 2015-present Mattermost, Inc.
// Licensed under AGPL-3.0.

use std::sync::LazyLock;

use regex::Regex;

static PUNC_START: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"^[^\pL\d\s#"]+"#).unwrap());
static PUNC_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"[^\pL\p{M}\d\s*"]+$"#).unwrap());
static HASHTAG_START: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#{2,}").unwrap());
static VALID_HASHTAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(#\pL[\pL\d\-_.]*[\pL\d])$").unwrap());

const FLAGS: [&str; 7] = ["from", "channel", "in", "before", "after", "on", "ext"];

#[derive(Debug, Default, Clone, PartialEq)]
pub struct SearchParams {
    pub terms: String,
    pub excluded_terms: String,
    pub is_hashtag: bool,
    pub in_channels: Vec<String>,
    pub excluded_channels: Vec<String>,
    pub from_users: Vec<String>,
    pub excluded_users: Vec<String>,
    pub after_date: String,
    pub excluded_after_date: String,
    pub before_date: String,
    pub excluded_before_date: String,
    pub extensions: Vec<String>,
    pub excluded_extensions: Vec<String>,
    pub on_date: String,
    pub excluded_date: String,
    pub or_terms: bool,
    pub include_deleted_channels: bool,
    /// Seconds east of UTC.
    pub time_zone_offset: i64,
    pub search_without_user_id: bool,
}

/// `splitWords`: whitespace-separated words, quoted phrases (with a leading `-`) kept whole.
fn split_words(text: &str) -> Vec<String> {
    let mut words = vec![];
    let mut found_quote = false;
    let mut location = 0;
    for (i, c) in text.char_indices() {
        if c != '"' {
            continue;
        }
        if found_quote {
            words.push(text[location..=i].to_string());
            found_quote = false;
            location = i + 1;
        } else {
            let next = if i > 0 && text.as_bytes()[i - 1] == b'-' { i - 1 } else { i };
            words.extend(text[location..next].split_whitespace().map(str::to_string));
            found_quote = true;
            location = next;
        }
    }
    words.extend(text[location..].split_whitespace().map(str::to_string));
    words
}

struct Flag {
    name: &'static str,
    value: String,
    exclude: bool,
}

fn parse_flags(input: &[String]) -> (Vec<(String, bool)>, Vec<Flag>) {
    let mut words = vec![];
    let mut flags = vec![];
    let mut skip = false;
    for (i, word) in input.iter().enumerate() {
        if skip {
            skip = false;
            continue;
        }
        let mut is_flag = false;
        if let Some(colon) = word.find(':') {
            let (name, exclude) =
                if let Some(w) = word.strip_prefix('-') { (&w[..colon - 1], true) } else { (&word[..colon], false) };
            let value = &word[colon + 1..];
            if let Some(f) = FLAGS.iter().find(|f| f.eq_ignore_ascii_case(name)) {
                if !value.is_empty() {
                    flags.push(Flag { name: f, value: value.into(), exclude });
                    is_flag = true;
                } else if i + 1 < input.len() {
                    flags.push(Flag { name: f, value: input[i + 1].clone(), exclude });
                    skip = true;
                    is_flag = true;
                }
            }
        }
        if !is_flag {
            let exclude = word.starts_with('-');
            let w = PUNC_START.replace_all(word, "");
            let w = PUNC_END.replace_all(&w, "");
            let w = HASHTAG_START.replace_all(&w, "#");
            if !w.is_empty() {
                words.push((w.into_owned(), exclude));
            }
        }
    }
    (words, flags)
}

/// `ParseSearchParams`: one set for plain terms, one for hashtags, or one for flags alone.
pub fn parse(text: &str, time_zone_offset: i64) -> Vec<SearchParams> {
    let (words, flags) = parse_flags(&split_words(text));
    let (mut tags, mut xtags, mut plain, mut xplain) = (vec![], vec![], vec![], vec![]);
    for (w, ex) in words {
        match (VALID_HASHTAG.is_match(&w), ex) {
            (true, true) => xtags.push(w),
            (true, false) => tags.push(w),
            (false, true) => xplain.push(w),
            (false, false) => plain.push(w),
        }
    }
    let mut base = SearchParams { time_zone_offset, ..Default::default() };
    for f in flags {
        let v = f.value;
        match (f.name, f.exclude) {
            ("in" | "channel", true) => base.excluded_channels.push(v),
            ("in" | "channel", false) => base.in_channels.push(v),
            ("from", true) => base.excluded_users.push(v),
            ("from", false) => base.from_users.push(v),
            ("after", true) => base.excluded_after_date = v,
            ("after", false) => base.after_date = v,
            ("before", true) => base.excluded_before_date = v,
            ("before", false) => base.before_date = v,
            ("on", true) => base.excluded_date = v,
            ("on", false) => base.on_date = v,
            ("ext", true) => base.excluded_extensions.push(v),
            ("ext", false) => base.extensions.push(v),
            _ => {}
        }
    }
    let (plain, xplain, tags, xtags) = (plain.join(" "), xplain.join(" "), tags.join(" "), xtags.join(" "));
    let mut out = vec![];
    if !plain.is_empty() || !xplain.is_empty() {
        out.push(SearchParams { terms: plain.clone(), excluded_terms: xplain.clone(), ..base.clone() });
    }
    if !tags.is_empty() || !xtags.is_empty() {
        out.push(SearchParams { terms: tags.clone(), excluded_terms: xtags.clone(), is_hashtag: true, ..base.clone() });
    }
    let has_flags = !base.in_channels.is_empty()
        || !base.from_users.is_empty()
        || !base.excluded_channels.is_empty()
        || !base.excluded_users.is_empty()
        || !base.extensions.is_empty()
        || !base.excluded_extensions.is_empty()
        || !base.after_date.is_empty()
        || !base.excluded_after_date.is_empty()
        || !base.before_date.is_empty()
        || !base.excluded_before_date.is_empty()
        || !base.on_date.is_empty()
        || !base.excluded_date.is_empty();
    if plain.is_empty() && tags.is_empty() && xplain.is_empty() && xtags.is_empty() && has_flags {
        out.push(base);
    }
    out
}

// ---------- dates ----------

/// `time.Parse("2006-01-02", PadDateStringZeros(s))` as days since the epoch.
fn parse_day(s: &str) -> Option<i64> {
    let parts: Vec<&str> = s.split('-').collect();
    let [y, m, d] = parts[..] else { return None };
    let pad = |s: &str| if s.len() == 1 { format!("0{s}") } else { s.to_string() };
    let (m, d) = (pad(m), pad(d));
    if y.len() != 4 || m.len() != 2 || d.len() != 2 {
        return None;
    }
    let (y, m, d): (i64, u32, u32) = (y.parse().ok()?, m.parse().ok()?, d.parse().ok()?);
    if !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) {
        return None;
    }
    Some(days_from_civil(y, m, d))
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        2 if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Howard Hinnant's (year, month, day) → days since 1970-01-01.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn today() -> i64 {
    crate::app::now_ms().div_euclid(86_400_000)
}

impl SearchParams {
    fn start(&self, day: i64) -> i64 {
        day * 86_400_000 - self.time_zone_offset * 1000
    }
    fn end(&self, day: i64) -> i64 {
        self.start(day) + 86_400_000 - 1
    }
    /// `GetAfterDateMillis`: from the start of the next day.
    pub fn after_millis(&self) -> i64 {
        self.start(parse_day(&self.after_date).unwrap_or_else(today) + 1)
    }
    pub fn excluded_after_millis(&self) -> i64 {
        self.start(parse_day(&self.excluded_after_date).unwrap_or_else(today) + 1)
    }
    /// `GetBeforeDateMillis`: up to the end of the day before.
    pub fn before_millis(&self) -> i64 {
        parse_day(&self.before_date).map_or(0, |d| self.end(d - 1))
    }
    pub fn excluded_before_millis(&self) -> i64 {
        parse_day(&self.excluded_before_date).map_or(0, |d| self.end(d - 1))
    }
    pub fn on_millis(&self) -> (i64, i64) {
        parse_day(&self.on_date).map_or((0, 0), |d| (self.start(d), self.end(d)))
    }
    pub fn excluded_date_millis(&self) -> (i64, i64) {
        parse_day(&self.excluded_date).map_or((0, 0), |d| (self.start(d), self.end(d)))
    }
}

/// `removeNonAlphaNumericUnquotedTerms`: words with no letter or digit go, unless quoted.
pub fn remove_non_alphanumeric_unquoted(line: &str, sep: &str) -> String {
    line.split(sep)
        .filter(|w| (w.len() >= 2 && w.starts_with('"') && w.ends_with('"')) || w.chars().any(char::is_alphanumeric))
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(sep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_terms_and_hashtags() {
        let p = parse("hello -world in:town-square from:@bob #tag ext:pdf after:2024-1-5", 0);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].terms, "hello");
        assert_eq!(p[0].excluded_terms, "world");
        assert_eq!(p[0].in_channels, vec!["town-square"]);
        assert_eq!(p[0].from_users, vec!["@bob"]);
        assert_eq!(p[0].extensions, vec!["pdf"]);
        assert_eq!(p[1].terms, "#tag");
        assert!(p[1].is_hashtag);
        assert_eq!(p[0].after_millis(), days_from_civil(2024, 1, 6) * 86_400_000);
        let only = parse("in: general", 0);
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].in_channels, vec!["general"]);
        assert_eq!(parse(r#"say "two words" -"not this""#, 0)[0].terms, r#"say "two words""#);
    }

    #[test]
    fn dates() {
        let p = SearchParams { on_date: "2024-03-01".into(), time_zone_offset: 3600, ..Default::default() };
        let (s, e) = p.on_millis();
        assert_eq!(s, days_from_civil(2024, 3, 1) * 86_400_000 - 3_600_000);
        assert_eq!(e - s, 86_400_000 - 1);
    }
}
