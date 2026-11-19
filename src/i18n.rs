//! Mattermost's English strings (`i18n/en.json`, verbatim from the 12.1.0 server): the messages of API errors,
//! looked up by error id as Go's `AppError.Translate` does with the default locale.

// i18n/en.json: Copyright (c) 2015-present Mattermost, Inc. Licensed under AGPL-3.0.

use std::collections::HashMap;
use std::sync::OnceLock;

static EN: &str = include_str!("../i18n/en.json");

fn table() -> &'static HashMap<&'static str, String> {
    static T: OnceLock<HashMap<&'static str, String>> = OnceLock::new();
    T.get_or_init(|| {
        #[derive(serde::Deserialize)]
        struct Entry<'a> {
            id: &'a str,
            translation: serde_json::Value,
        }
        let entries: Vec<Entry<'static>> = serde_json::from_str(EN).unwrap_or_default();
        entries
            .into_iter()
            .filter_map(|e| {
                // Plural forms are objects ({"one", "other"}); "other" serves.
                let t = match e.translation {
                    serde_json::Value::String(s) => s,
                    serde_json::Value::Object(m) => m.get("other")?.as_str()?.to_string(),
                    _ => return None,
                };
                Some((e.id, t))
            })
            .collect()
    })
}

/// The English text of `id`, if it needs no parameters.
pub fn t(id: &str) -> Option<&'static str> {
    table().get(id).map(String::as_str).filter(|s| !s.contains("{{"))
}

/// The English text of `id` with `{{.Name}}` parameters filled in.
pub fn tr(id: &str, args: &[(&str, &str)]) -> Option<String> {
    let mut s = table().get(id)?.clone();
    for (k, v) in args {
        s = s.replace(&format!("{{{{.{k}}}}}"), v);
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    #[test]
    fn looks_up_by_id() {
        assert_eq!(super::t("api.context.session_expired.app_error"), Some("Invalid or expired session, please login again."));
        assert!(super::tr("api.context.invalid_body_param.app_error", &[("Name", "x")]).unwrap().contains(" x "));
    }
}
