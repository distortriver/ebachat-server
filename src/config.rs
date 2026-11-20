//! Mattermost's `config.json` (or the active row of `Configurations`), with `MM_SECTION_KEY` environment
//! overrides, and the client config the API sends from it.

// Based on Mattermost server/config/client.go and config/environment.go, Copyright (c) 2015-present
// Mattermost, Inc. Licensed under AGPL-3.0.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicPtr, Ordering};

use serde_json::{Map, Value};

/// The server's settings, as `config.Store` keeps them: what the source holds (`raw`), and in effect, the same
/// with the `MM_*` environment applied. A save or reload swaps the settings in effect at once; readers keep
/// borrowing `&'static` values, so a replaced version is never freed (a config save is a rare admin action).
pub struct Config {
    cur: AtomicPtr<Value>,
    raw: Mutex<Value>,
    /// `Section.Key` paths the environment sets (`GetEnvironmentConfig`).
    env_paths: Mutex<Vec<String>>,
    /// `MM_CONFIG`: a file, or a Postgres DSN for the `Configurations` table.
    source: String,
    /// One save or reload at a time.
    saving: tokio::sync::Mutex<()>,
    pub diagnostic_id: String,
}

/// `model.FakeSetting`: what a secret reads as.
pub const FAKE_SETTING: &str = "********************************";

/// `SaveConfig`'s error when the store refuses.
fn save_error(detail: String) -> crate::app::AppError {
    crate::app::AppError::internal("app.save_config.app_error", detail)
}

fn leak(v: Value) -> *mut Value {
    Box::into_raw(Box::new(v))
}

impl Config {
    /// `MM_CONFIG`: a file (default `config/config.json`) or a Postgres DSN for the `Configurations` table.
    pub async fn load() -> anyhow::Result<Config> {
        let src = std::env::var("MM_CONFIG").unwrap_or_else(|_| "config/config.json".into());
        let raw = read_source(&src).await?;
        Ok(Config::from_raw(raw, src))
    }

    /// Settings from what a source holds (tests, and `load`): Go's defaults where it has none, as `Store.Load`.
    pub fn from_raw(raw: Value, source: String) -> Config {
        let raw = with_defaults(&raw);
        let (v, paths) = with_env(&raw);
        Config {
            cur: AtomicPtr::new(leak(v)),
            raw: Mutex::new(raw),
            env_paths: Mutex::new(paths),
            source,
            saving: tokio::sync::Mutex::new(()),
            diagnostic_id: String::new(),
        }
    }

    /// Settings exactly as given (Go's defaults where they have none), without the environment: what a
    /// connection test checks.
    pub fn from_value(v: Value) -> Config {
        let raw = with_defaults(&v);
        Config {
            cur: AtomicPtr::new(leak(raw.clone())),
            raw: Mutex::new(raw),
            env_paths: Mutex::new(vec![]),
            source: String::new(),
            saving: tokio::sync::Mutex::new(()),
            diagnostic_id: String::new(),
        }
    }

    /// The settings in effect.
    pub fn value(&self) -> &'static Value {
        // SAFETY: every pointer stored is a leaked Box, never freed.
        unsafe { &*self.cur.load(Ordering::Acquire) }
    }

    fn install(&self, raw: Value) {
        let raw = with_defaults(&raw);
        let (v, paths) = with_env(&raw);
        *self.raw.lock().unwrap() = raw;
        *self.env_paths.lock().unwrap() = paths;
        self.cur.store(leak(v), Ordering::Release);
        crate::app::DEVELOPER.store(self.bool("ServiceSettings.EnableDeveloper"), Ordering::Relaxed);
    }

    /// `config.Store.Set`: `new` with defaults where it has none and its masked secrets as they were, checked,
    /// written to the source without what the environment sets (and without feature flags), then in effect.
    /// Returns the settings in effect before and after.
    pub async fn save(&self, new: Value) -> Result<(&'static Value, &'static Value), crate::app::AppError> {
        let _one = self.saving.lock().await;
        let old = self.value();
        let mut raw = new;
        fill_defaults(defaults(), &mut raw);
        desanitize(old, &mut raw);
        let (in_effect, _) = with_env(&raw);
        if let Err(e) = crate::config_valid::is_valid(&in_effect) {
            return Err(save_error(e.message));
        }
        // removeEnvOverrides: what the environment sets stays as the source had it.
        let stored = self.raw.lock().unwrap().clone();
        let paths = self.env_paths.lock().unwrap().clone();
        for p in &paths {
            let keys: Vec<&str> = p.split('.').collect();
            match lookup(&stored, &keys) {
                Some(v) => set_at(&mut raw, &keys, v.clone()),
                None => remove_at(&mut raw, &keys),
            }
        }
        fix(&mut raw);
        let mut persisted = raw.clone();
        if let Some(m) = persisted.as_object_mut() {
            m.remove("FeatureFlags");
        }
        write_source(&self.source, &persisted).await.map_err(|e| save_error(e.to_string()))?;
        self.install(raw);
        Ok((old, self.value()))
    }

    /// `ReloadConfig`: the source read again. Returns the settings in effect before and after.
    pub async fn reload(&self) -> anyhow::Result<(&'static Value, &'static Value)> {
        let _one = self.saving.lock().await;
        let old = self.value();
        let raw = read_source(&self.source).await?;
        self.install(raw);
        Ok((old, self.value()))
    }

    /// `GetEnvironmentConfig`: `{Section: {Key: true}}` for the settings the environment sets.
    pub fn environment(&self) -> Value {
        let mut out = Value::Object(Map::new());
        for p in self.env_paths.lock().unwrap().iter() {
            let keys: Vec<&str> = p.split('.').collect();
            set_at(&mut out, &keys, Value::Bool(true));
        }
        out
    }

    pub fn get(&self, path: &str) -> &'static Value {
        static NULL: Value = Value::Null;
        let mut v = self.value();
        for k in path.split('.') {
            v = v.get(k).unwrap_or(&NULL);
        }
        v
    }
    pub fn str(&self, path: &str) -> &'static str {
        self.get(path).as_str().unwrap_or("")
    }
    pub fn bool(&self, path: &str) -> bool {
        self.get(path).as_bool().unwrap_or(false)
    }
    pub fn i64(&self, path: &str) -> i64 {
        self.get(path).as_i64().unwrap_or(0)
    }
    /// A boolean feature flag: the config's `FeatureFlags.<name>`, else the 12.1.0 default.
    pub fn feature(&self, name: &str) -> bool {
        match self.get(&format!("FeatureFlags.{name}")) {
            Value::Bool(b) => *b,
            Value::String(s) => s == "true",
            _ => FEATURE_FLAGS.iter().any(|(k, v)| *k == name && *v == "true"),
        }
    }

    /// `Config.Sanitize` without plugin manifests (plugins are off): secrets masked, plugin settings dropped.
    pub fn sanitized(&self) -> Value {
        sanitize(self.value().clone())
    }

    /// `GenerateClientConfig` for a server without a license.
    pub fn client(&self, limited: bool) -> BTreeMap<String, String> {
        let mut p = BTreeMap::new();
        let mut put = |k: &str, v: String| {
            p.insert(k.to_string(), v);
        };
        for (k, v) in [
            ("Version", crate::SERVER_VERSION_FULL),
            ("BuildNumber", crate::SERVER_VERSION),
            ("BuildDate", ""),
            ("BuildHash", crate::BUILD_HASH),
            ("BuildHashEnterprise", ""),
            ("BuildEnterpriseReady", "false"),
            ("IsFipsEnabled", "false"),
        ] {
            put(k, (*v).into());
        }
        let render = |v: &Value| match v {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            Value::Array(a) => a.iter().map(|x| x.as_str().unwrap_or_default()).collect::<Vec<_>>().join(","),
            v => v.to_string(),
        };
        for (k, path) in LIMITED {
            put(k, render(self.get(path)));
        }
        put("SiteURL", self.str("ServiceSettings.SiteURL").trim_end_matches('/').into());
        put("CWSURL", String::new());
        put("WebsocketURL", self.str("ServiceSettings.WebsocketURL").trim_end_matches('/').into());
        put("DiagnosticId", self.diagnostic_id.clone());
        put("TelemetryId", self.diagnostic_id.clone());
        put("AppsPluginEnabled", "false".into());
        // GetServiceEnvironment: production unless the environment says otherwise.
        let env = std::env::var("MM_SERVICEENVIRONMENT").unwrap_or_default().to_lowercase();
        put("ServiceEnvironment", if env == "dev" || env == "test" { env } else { "production".into() });
        // FeatureFlags.ToMap: the 12.1.0 defaults, overridden by the config's FeatureFlags.
        for (k, v) in FEATURE_FLAGS {
            let set = self.get(&format!("FeatureFlags.{k}"));
            put(&format!("FeatureFlag{k}"), if set.is_null() { (*v).to_string() } else { render(set) });
        }
        for (k, v) in LIMITED_FIXED {
            put(k, (*v).into());
        }
        if limited {
            return p;
        }
        for (k, path) in FULL {
            put(k, render(self.get(path)));
        }
        for (k, v) in FULL_FIXED {
            put(k, (*v).into());
        }
        put("CWSURL", self.str("CloudSettings.CWSURL").into());
        // getGiphySdkKey: the configured key in production (our builds embed none), GIPHY's test key otherwise.
        let env = std::env::var("MM_SERVICEENVIRONMENT").unwrap_or_default().to_lowercase();
        let giphy = if env == "dev" || env == "test" {
            "s0glxvzVg9azvPipKxcPLpXV0q1x1fVP"
        } else {
            self.str("ServiceSettings.GiphySdkKey")
        };
        put("GiphySdkKey", giphy.into());
        put("IsDefaultMarketplace", "true".into());
        p
    }
}

/// `MM_SERVICESETTINGS_SITEURL=...`: a setting by its section and name, case-insensitive, typed as the value
/// it replaces. Returns the `Section.Key` paths set.
fn apply_env(v: &mut Value, vars: impl Iterator<Item = (String, String)>) -> Vec<String> {
    let mut paths = vec![];
    for (k, val) in vars {
        let Some(rest) = k.strip_prefix("MM_") else { continue };
        if let Some(p) = set_path(v, rest, &val) {
            paths.push(p);
        }
    }
    paths.sort();
    paths.dedup();
    paths
}

fn set_path(v: &mut Value, key: &str, val: &str) -> Option<String> {
    let m = v.as_object_mut()?;
    // Section names have no underscores; walk down matching `SECTION_` prefixes.
    let names: Vec<String> = m.keys().cloned().collect();
    for name in names {
        let up = name.to_uppercase();
        if key == up {
            let old = &m[&name];
            if let Some(t) = typed(old, val) {
                m.insert(name.clone(), t);
            }
            return Some(name);
        }
        if let Some(rest) = key.strip_prefix(&format!("{up}_"))
            && m[&name].is_object()
        {
            return set_path(m.get_mut(&name).unwrap(), rest, val).map(|p| format!("{name}.{p}"));
        }
    }
    None
}

async fn read_source(src: &str) -> anyhow::Result<Value> {
    if src.starts_with("postgres") {
        let db = sqlx::PgPool::connect(&crate::db_url(src)).await?;
        let (s,): (String,) = sqlx::query_as("SELECT Value FROM Configurations WHERE Active").fetch_one(&db).await?;
        Ok(serde_json::from_str(&s)?)
    } else {
        Ok(serde_json::from_slice(&std::fs::read(src).map_err(|e| anyhow::anyhow!("{src}: {e}"))?)?)
    }
}

/// `FileStore.persist` (4-space JSON, as Go writes it) or `DatabaseStore.persist` (a new active row).
async fn write_source(src: &str, v: &Value) -> anyhow::Result<()> {
    if src.starts_with("postgres") {
        let db = sqlx::PgPool::connect(&crate::db_url(src)).await?;
        // marshalConfig: MarshalIndent, as for the file.
        let s = crate::config_order::to_go_json(v, true);
        let sha = {
            use sha2::Digest;
            let d = sha2::Sha256::digest(s.as_bytes());
            d.iter().map(|b| format!("{b:02x}")).collect::<String>()
        };
        let mut tx = db.begin().await?;
        sqlx::query("UPDATE Configurations SET Active = NULL WHERE Active").execute(&mut *tx).await?;
        sqlx::query("INSERT INTO Configurations (Id, Value, CreateAt, Active, SHA) VALUES ($1, $2, $3, true, $4)")
            .bind(crate::app::new_id())
            .bind(&s)
            .bind(crate::app::now_ms())
            .bind(sha)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Ok(());
    }
    // Go's MarshalIndent of model.Config: the struct's field order.
    let buf = crate::config_order::to_go_json(v, true);
    let tmp = format!("{src}.tmp");
    std::fs::write(&tmp, buf.as_bytes())?;
    std::fs::rename(&tmp, src)?;
    Ok(())
}

fn lookup<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().try_fold(v, |v, k| v.get(*k))
}

fn set_at(v: &mut Value, keys: &[&str], val: Value) {
    let Some((last, head)) = keys.split_last() else { return };
    let mut cur = v;
    for k in head {
        if !cur.get(*k).is_some_and(Value::is_object)
            && let Some(m) = cur.as_object_mut()
        {
            m.insert(k.to_string(), Value::Object(Map::new()));
        }
        let Some(next) = cur.get_mut(*k) else { return };
        cur = next;
    }
    if let Some(m) = cur.as_object_mut() {
        m.insert(last.to_string(), val);
    }
}

fn remove_at(v: &mut Value, keys: &[&str]) {
    let Some((last, head)) = keys.split_last() else { return };
    let mut cur = v;
    for k in head {
        let Some(n) = cur.get_mut(*k) else { return };
        cur = n;
    }
    if let Some(m) = cur.as_object_mut() {
        m.remove(*last);
    }
}

/// The secrets `Config.Sanitize` masks: (path, masked only when not empty).
const SECRETS: &[(&str, bool)] = &[
    ("LdapSettings.BindPassword", true),
    ("FileSettings.PublicLinkSalt", false),
    ("FileSettings.AmazonS3SecretAccessKey", true),
    ("FileSettings.ExportAmazonS3SecretAccessKey", true),
    ("FileSettings.AzureAccessKey", true),
    ("FileSettings.ExportAzureAccessKey", true),
    ("EmailSettings.SMTPPassword", true),
    ("GitLabSettings.Secret", true),
    ("GoogleSettings.Secret", true),
    ("Office365Settings.Secret", true),
    ("OpenIdSettings.Secret", true),
    ("SqlSettings.DataSource", false),
    ("ElasticsearchSettings.Password", false),
    ("MessageExportSettings.GlobalRelaySettings.SMTPPassword", true),
    ("ServiceSettings.SplitKey", false),
    ("ServiceSettings.GoogleDeveloperKey", true),
    ("ServiceSettings.GiphySdkKey", true),
    ("CacheSettings.RedisPassword", false),
    ("AutoTranslationSettings.LibreTranslate.APIKey", true),
];

/// `Config.Sanitize(nil manifests)`.
pub fn sanitize(mut v: Value) -> Value {
    for (path, only_set) in SECRETS {
        let keys: Vec<&str> = path.split('.').collect();
        match lookup(&v, &keys) {
            Some(Value::String(s)) if *only_set && s.is_empty() => {}
            Some(Value::Null) | None => {}
            Some(_) => set_at(&mut v, &keys, Value::String(FAKE_SETTING.into())),
        }
    }
    for list in ["DataSourceReplicas", "DataSourceSearchReplicas"] {
        if let Some(Value::Array(a)) = v.pointer_mut(&format!("/SqlSettings/{list}")) {
            for x in a.iter_mut() {
                *x = Value::String(FAKE_SETTING.into());
            }
        }
    }
    if let Some(Value::Array(a)) = v.pointer_mut("/SqlSettings/ReplicaLagSettings") {
        for x in a.iter_mut() {
            if let Some(d) = x.get_mut("DataSource").filter(|d| !d.is_null()) {
                *d = Value::String(FAKE_SETTING.into());
            }
        }
    }
    // PluginSettings.Sanitize: settings of plugins not installed are not returned.
    if let Some(Value::Object(p)) = v.pointer_mut("/PluginSettings/Plugins") {
        p.retain(|_, s| s.as_object().is_some_and(|m| m.is_empty()));
    }
    v
}

/// `desanitize`: a masked secret sent back means "unchanged".
pub fn desanitize(old: &Value, new: &mut Value) {
    for (path, _) in SECRETS {
        let keys: Vec<&str> = path.split('.').collect();
        if lookup(new, &keys).and_then(Value::as_str) == Some(FAKE_SETTING) {
            match lookup(old, &keys) {
                Some(v) => set_at(new, &keys, v.clone()),
                None => remove_at(new, &keys),
            }
        }
    }
    for list in ["DataSourceReplicas", "DataSourceSearchReplicas"] {
        let p = format!("/SqlSettings/{list}");
        if let (Some(Value::Array(o)), Some(Value::Array(n))) = (old.pointer(&p), new.pointer(&p).cloned()) {
            let fixed: Vec<Value> = n
                .iter()
                .enumerate()
                .map(|(i, x)| {
                    if x.as_str() == Some(FAKE_SETTING) { o.get(i).cloned().unwrap_or(Value::Null) } else { x.clone() }
                })
                .collect();
            if let Some(slot) = new.pointer_mut(&p) {
                *slot = Value::Array(fixed);
            }
        }
    }
}

/// `config.Merge`: `patch` over `base`, objects merged key by key.
pub fn merge(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (k, v) in p {
                match b.get_mut(k) {
                    Some(bv) if bv.is_object() && v.is_object() => merge(bv, v),
                    _ => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (b, p) => *b = p.clone(),
    }
}

/// `applyEnvKey`: the value typed as the setting it replaces; `None` where Go's parse fails (left unchanged).
fn typed(old: &Value, s: &str) -> Option<Value> {
    Some(match old {
        // strconv.ParseBool
        Value::Bool(_) => Value::Bool(match s {
            "1" | "t" | "T" | "TRUE" | "true" | "True" => true,
            "0" | "f" | "F" | "FALSE" | "false" | "False" => false,
            _ => return None,
        }),
        Value::Number(_) => Value::from(s.parse::<i64>().ok()?),
        Value::Array(_) => Value::Array(s.split(' ').map(|x| Value::String(x.into())).collect()),
        Value::Object(_) => serde_json::from_str::<Value>(s).ok().filter(Value::is_object)?,
        _ => Value::String(s.into()),
    })
}

/// Go's `model.Config` after `SetDefaults` (`data/config_defaults.json`, made by `tools/gomodel`).
pub fn defaults() -> &'static Value {
    static D: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    D.get_or_init(|| serde_json::from_str(include_str!("../data/config_defaults.json")).expect("config defaults"))
}

/// The `access` tag of every setting, by path (`data/config_access.json`, made by `tools/gomodel`).
pub fn access_tags() -> &'static BTreeMap<String, String> {
    static A: std::sync::OnceLock<BTreeMap<String, String>> = std::sync::OnceLock::new();
    A.get_or_init(|| serde_json::from_str(include_str!("../data/config_access.json")).expect("config access tags"))
}

/// A section or setting struct (`model.Config` has no empty structs); an empty object is a map setting.
fn is_struct(schema: &Value) -> bool {
    schema.as_object().is_some_and(|m| !m.is_empty())
}

/// `json.Unmarshal` into `*model.Config`: what Go does not know is dropped, `null` stays nil, and a value of
/// another type than the setting's is an error (its path). `lenient` (a stored config) drops such a value instead.
pub fn conform(schema: &Value, v: &Value, lenient: bool) -> Result<Value, String> {
    fn go(schema: &Value, v: &Value, lenient: bool, path: &str) -> Result<Option<Value>, String> {
        let ok = match (schema, v) {
            (_, Value::Null) => true,
            (Value::Object(sm), Value::Object(vm)) if !sm.is_empty() => {
                let mut out = Map::new();
                for (k, x) in vm {
                    if let Some(s) = sm.get(k)
                        && let Some(c) = go(s, x, lenient, &format!("{path}{k}."))?
                    {
                        out.insert(k.clone(), c);
                    }
                }
                return Ok(Some(Value::Object(out)));
            }
            (Value::Object(_), Value::Object(_)) | (Value::Null, _) => true,
            (Value::Bool(_), Value::Bool(_)) | (Value::String(_), Value::String(_)) | (Value::Array(_), Value::Array(_)) => true,
            // Go's settings are integers.
            (Value::Number(_), Value::Number(n)) => n.is_i64() || n.is_u64(),
            _ => false,
        };
        if ok {
            Ok(Some(v.clone()))
        } else if lenient {
            Ok(None)
        } else {
            Err(path.trim_end_matches('.').to_string())
        }
    }
    Ok(go(schema, v, lenient, "")?.unwrap_or(Value::Null))
}

/// `Config.SetDefaults` over a decoded config: every setting nil or missing gets its default.
pub fn fill_defaults(schema: &Value, v: &mut Value) {
    if !v.is_object() {
        *v = schema.clone();
        return;
    }
    let (Some(sm), Some(vm)) = (schema.as_object(), v.as_object_mut()) else { return };
    for (k, s) in sm {
        match vm.get_mut(k) {
            None | Some(Value::Null) => {
                vm.insert(k.clone(), s.clone());
            }
            Some(x) if is_struct(s) => fill_defaults(s, x),
            Some(_) => {}
        }
    }
}

/// What a stored config is in Go once loaded: decoded into `model.Config`, defaults set, `fixConfig`.
fn with_defaults(raw: &Value) -> Value {
    let mut v = conform(defaults(), raw, true).unwrap_or(Value::Null);
    fill_defaults(defaults(), &mut v);
    fix(&mut v);
    v
}

/// The settings in effect: the stored ones with the `MM_*` environment, and the paths it sets.
fn with_env(raw: &Value) -> (Value, Vec<String>) {
    let mut v = raw.clone();
    let paths = apply_env(&mut v, std::env::vars());
    fix(&mut v);
    (v, paths)
}

/// `fixConfig`: the site URL without a trailing slash, a local file directory with one.
fn fix(v: &mut Value) {
    if let Some(Value::String(u)) = v.pointer_mut("/ServiceSettings/SiteURL") {
        *u = u.trim_end_matches('/').to_string();
    }
    if v.pointer("/FileSettings/DriverName").and_then(Value::as_str) == Some("local")
        && let Some(Value::String(d)) = v.pointer_mut("/FileSettings/Directory")
        && !d.is_empty()
        && !d.ends_with('/')
    {
        d.push('/');
    }
}

/// `config.Merge(base, patch)`: settings `patch` sets (not nil) replace `base`'s; sections merge setting by
/// setting, a map setting is replaced whole. `keep(path)` false leaves a setting as `base` has it (the write
/// filter).
pub fn merge_cfg(schema: &Value, base: &mut Value, patch: &Value, keep: &dyn Fn(&str) -> bool) {
    fn go(schema: &Value, base: &mut Value, patch: &Value, keep: &dyn Fn(&str) -> bool, path: &str) {
        let (Some(sm), Some(pm)) = (schema.as_object(), patch.as_object()) else { return };
        if !base.is_object() {
            *base = Value::Object(Map::new());
        }
        let Some(bm) = base.as_object_mut() else { return };
        for (k, p) in pm {
            let Some(s) = sm.get(k) else { continue };
            if p.is_null() {
                continue;
            }
            let full = format!("{path}{k}");
            if is_struct(s) {
                go(s, bm.entry(k.clone()).or_insert(Value::Null), p, keep, &format!("{full}."));
            } else if keep(&full) {
                bm.insert(k.clone(), p.clone());
            }
        }
    }
    go(schema, base, patch, keep, "")
}

const LIMITED: &[(&str, &str)] = &[
    ("EnableBotAccountCreation", "ServiceSettings.EnableBotAccountCreation"),
    ("EnableDesktopLandingPage", "ServiceSettings.EnableDesktopLandingPage"),
    ("EnableFile", "LogSettings.EnableFile"),
    ("FileLevel", "LogSettings.FileLevel"),
    ("SiteName", "TeamSettings.SiteName"),
    ("WebsocketPort", "ServiceSettings.WebsocketPort"),
    ("WebsocketSecurePort", "ServiceSettings.WebsocketSecurePort"),
    ("EnableUserCreation", "TeamSettings.EnableUserCreation"),
    ("EnableOpenServer", "TeamSettings.EnableOpenServer"),
    ("EnableJoinLeaveMessageByDefault", "TeamSettings.EnableJoinLeaveMessageByDefault"),
    ("AndroidLatestVersion", "ClientRequirements.AndroidLatestVersion"),
    ("AndroidMinVersion", "ClientRequirements.AndroidMinVersion"),
    ("IosLatestVersion", "ClientRequirements.IosLatestVersion"),
    ("IosMinVersion", "ClientRequirements.IosMinVersion"),
    ("EnableDiagnostics", "LogSettings.EnableDiagnostics"),
    ("EnableClientMetrics", "MetricsSettings.EnableClientMetrics"),
    ("EnableComplianceExport", "MessageExportSettings.EnableExport"),
    ("EnableSignUpWithEmail", "EmailSettings.EnableSignUpWithEmail"),
    ("EnableSignInWithEmail", "EmailSettings.EnableSignInWithEmail"),
    ("EnableSignInWithUsername", "EmailSettings.EnableSignInWithUsername"),
    ("TermsOfServiceLink", "SupportSettings.TermsOfServiceLink"),
    ("PrivacyPolicyLink", "SupportSettings.PrivacyPolicyLink"),
    ("AboutLink", "SupportSettings.AboutLink"),
    ("HelpLink", "SupportSettings.HelpLink"),
    ("ReportAProblemType", "SupportSettings.ReportAProblemType"),
    ("ReportAProblemLink", "SupportSettings.ReportAProblemLink"),
    ("ReportAProblemMail", "SupportSettings.ReportAProblemMail"),
    ("AllowDownloadLogs", "SupportSettings.AllowDownloadLogs"),
    ("ForgotPasswordLink", "SupportSettings.ForgotPasswordLink"),
    ("SupportEmail", "SupportSettings.SupportEmail"),
    ("EnableAskCommunityLink", "SupportSettings.EnableAskCommunityLink"),
    ("DefaultClientLocale", "LocalizationSettings.DefaultClientLocale"),
    ("EnableCustomEmoji", "ServiceSettings.EnableCustomEmoji"),
    ("EnableUserStatuses", "ServiceSettings.EnableUserStatuses"),
    ("AppDownloadLink", "NativeAppSettings.AppDownloadLink"),
    ("AndroidAppDownloadLink", "NativeAppSettings.AndroidAppDownloadLink"),
    ("IosAppDownloadLink", "NativeAppSettings.IosAppDownloadLink"),
    ("MobileExternalBrowser", "NativeAppSettings.MobileExternalBrowser"),
    ("DiagnosticsEnabled", "LogSettings.EnableDiagnostics"),
    ("HasImageProxy", "ImageProxySettings.Enable"),
    ("PluginsEnabled", "PluginSettings.Enable"),
    ("PasswordMinimumLength", "PasswordSettings.MinimumLength"),
    ("PasswordRequireLowercase", "PasswordSettings.Lowercase"),
    ("PasswordRequireUppercase", "PasswordSettings.Uppercase"),
    ("PasswordRequireNumber", "PasswordSettings.Number"),
    ("PasswordRequireSymbol", "PasswordSettings.Symbol"),
    ("PasswordEnableForgotLink", "PasswordSettings.EnableForgotLink"),
    ("EnableCustomBrand", "TeamSettings.EnableCustomBrand"),
    ("CustomBrandText", "TeamSettings.CustomBrandText"),
    ("CustomDescriptionText", "TeamSettings.CustomDescriptionText"),
    ("EnableMultifactorAuthentication", "ServiceSettings.EnableMultifactorAuthentication"),
    ("EnableGuestAccounts", "GuestAccountsSettings.Enable"),
    ("HideGuestTags", "GuestAccountsSettings.HideTags"),
    ("GuestAccountsEnforceMultifactorAuthentication", "GuestAccountsSettings.EnforceMultifactorAuthentication"),
    ("EnableGuestMagicLink", "GuestAccountsSettings.EnableGuestMagicLink"),
];

/// `model.FeatureFlags` defaults of 12.1.0.
const FEATURE_FLAGS: &[(&str, &str)] = &[
    ("AggregatePluginMetrics", "false"),
    ("AppsEnabled", "false"),
    ("AttributeValueMasking", "true"),
    ("AutoTranslation", "true"),
    ("BurnOnRead", "true"),
    ("CJKSearch", "true"),
    ("ChannelAttributes", "true"),
    ("ChannelAttributesRequired", "false"),
    ("ChannelPermissionPolicies", "true"),
    ("ClassificationMarkings", "true"),
    ("ClusterGracefulDrain", "true"),
    ("ContentFlagging", "true"),
    ("DiscoverableChannels", "false"),
    ("EnableAIPluginBridge", "false"),
    ("EnableAIRecaps", "false"),
    ("EnableDocs", "false"),
    ("EnableMFIPluginSignaturePublicKey", "true"),
    ("EnableMattermostEntry", "true"),
    ("EnableSharedChannelsDMs", "false"),
    ("EnableShiftEscapeToMarkAllRead", "false"),
    ("EnableSyncAllUsersForRemoteCluster", "false"),
    ("EnforceLogPathRoot", "false"),
    ("HealthDashboard", "false"),
    ("IntegratedBoards", "false"),
    ("ManagedChannelCategories", "false"),
    ("MmBlocksEnabled", "true"),
    ("MobileEphemeralMode", "false"),
    ("MobileSSOCodeExchange", "false"),
    ("MoveThreadsEnabled", "false"),
    ("NormalizeLdapDNs", "false"),
    ("NotificationMonitoring", "true"),
    ("OpenIdAttributeSync", "true"),
    ("PermissionPolicies", "true"),
    ("PolicySimulation", "true"),
    ("PostAttributes", "false"),
    ("PostDeliveryTracking", "false"),
    ("PropertyFieldGraph", "true"),
    ("PropertyFieldRank", "true"),
    ("RecurringScheduledPosts", "false"),
    ("ResourceAttributesInPolicies", "true"),
    ("SessionAttributes", "true"),
    ("TeamMembershipAccessControl", "true"),
    ("TestBoolFeature", "false"),
    ("TestFeature", "off"),
    ("WysiwygEditor", "false"),
];

const LIMITED_FIXED: &[(&str, &str)] = &[
    ("EnableLdap", "false"),
    ("LdapLoginFieldName", ""),
    ("EnableSaml", "false"),
    ("SamlLoginButtonText", ""),
    ("EnableSignUpWithGoogle", "false"),
    ("EnableSignUpWithOffice365", "false"),
    ("EnableSignUpWithOpenId", "false"),
    ("OpenIdButtonText", ""),
    ("OpenIdButtonColor", ""),
    ("EnforceMultifactorAuthentication", "false"),
];

const FULL: &[(&str, &str)] = &[
    ("EnableCustomUserStatuses", "TeamSettings.EnableCustomUserStatuses"),
    ("EnableLastActiveTime", "TeamSettings.EnableLastActiveTime"),
    ("EnableUserDeactivation", "TeamSettings.EnableUserDeactivation"),
    ("RestrictDirectMessage", "TeamSettings.RestrictDirectMessage"),
    ("TeammateNameDisplay", "TeamSettings.TeammateNameDisplay"),
    ("LockTeammateNameDisplay", "TeamSettings.LockTeammateNameDisplay"),
    ("ExperimentalPrimaryTeam", "TeamSettings.ExperimentalPrimaryTeam"),
    ("EnableChannelCategorySorting", "TeamSettings.EnableChannelCategorySorting"),
    ("EnableOAuthServiceProvider", "ServiceSettings.EnableOAuthServiceProvider"),
    ("GoogleDeveloperKey", "ServiceSettings.GoogleDeveloperKey"),
    ("EnableIncomingWebhooks", "ServiceSettings.EnableIncomingWebhooks"),
    ("EnableOutgoingWebhooks", "ServiceSettings.EnableOutgoingWebhooks"),
    ("EnableOutgoingOAuthConnections", "ServiceSettings.EnableOutgoingOAuthConnections"),
    ("EnableCommands", "ServiceSettings.EnableCommands"),
    ("EnablePostUsernameOverride", "ServiceSettings.EnablePostUsernameOverride"),
    ("EnablePostIconOverride", "ServiceSettings.EnablePostIconOverride"),
    ("EnableUserAccessTokens", "ServiceSettings.EnableUserAccessTokens"),
    ("MaximumPersonalAccessTokenLifetimeDays", "ServiceSettings.MaximumPersonalAccessTokenLifetimeDays"),
    ("EnableLinkPreviews", "ServiceSettings.EnableLinkPreviews"),
    ("EnablePermalinkPreviews", "ServiceSettings.EnablePermalinkPreviews"),
    ("EnableTesting", "ServiceSettings.EnableTesting"),
    ("EnableDeveloper", "ServiceSettings.EnableDeveloper"),
    ("EnableClientPerformanceDebugging", "ServiceSettings.EnableClientPerformanceDebugging"),
    ("PostEditTimeLimit", "ServiceSettings.PostEditTimeLimit"),
    ("MinimumHashtagLength", "ServiceSettings.MinimumHashtagLength"),
    ("EnableTutorial", "ServiceSettings.EnableTutorial"),
    ("EnableOnboardingFlow", "ServiceSettings.EnableOnboardingFlow"),
    (
        "ExperimentalEnableDefaultChannelLeaveJoinMessages",
        "ServiceSettings.ExperimentalEnableDefaultChannelLeaveJoinMessages",
    ),
    ("ExperimentalGroupUnreadChannels", "ServiceSettings.ExperimentalGroupUnreadChannels"),
    ("EnableSVGs", "ServiceSettings.EnableSVGs"),
    ("EnableMarketplace", "PluginSettings.EnableMarketplace"),
    ("EnableLatex", "ServiceSettings.EnableLatex"),
    ("EnableInlineLatex", "ServiceSettings.EnableInlineLatex"),
    ("ExtendSessionLengthWithActivity", "ServiceSettings.ExtendSessionLengthWithActivity"),
    ("ManagedResourcePaths", "ServiceSettings.ManagedResourcePaths"),
    ("DeleteAccountLink", "ServiceSettings.DeleteAccountLink"),
    ("DisableAppBar", "ExperimentalSettings.DisableAppBar"),
    ("ExperimentalEnableAutomaticReplies", "TeamSettings.ExperimentalEnableAutomaticReplies"),
    ("SendEmailNotifications", "EmailSettings.SendEmailNotifications"),
    ("SendPushNotifications", "EmailSettings.SendPushNotifications"),
    ("RequireEmailVerification", "EmailSettings.RequireEmailVerification"),
    ("EnableEmailBatching", "EmailSettings.EnableEmailBatching"),
    ("EnablePreviewModeBanner", "EmailSettings.EnablePreviewModeBanner"),
    ("EmailNotificationContentsType", "EmailSettings.EmailNotificationContentsType"),
    ("ShowEmailAddress", "PrivacySettings.ShowEmailAddress"),
    ("ShowFullName", "PrivacySettings.ShowFullName"),
    ("UseAnonymousURLs", "PrivacySettings.UseAnonymousURLs"),
    ("EnableFileAttachments", "FileSettings.EnableFileAttachments"),
    ("EnableFileSearch", "ServiceSettings.EnableFileSearch"),
    ("EnablePublicLink", "FileSettings.EnablePublicLink"),
    ("AvailableLocales", "LocalizationSettings.AvailableLocales"),
    ("SQLDriverName", "SqlSettings.DriverName"),
    ("EnableEmojiPicker", "ServiceSettings.EnableEmojiPicker"),
    ("EnableGifPicker", "ServiceSettings.EnableGifPicker"),
    ("MaxFileSize", "FileSettings.MaxFileSize"),
    ("MaxNotificationsPerChannel", "TeamSettings.MaxNotificationsPerChannel"),
    ("EnableConfirmNotificationsToChannel", "TeamSettings.EnableConfirmNotificationsToChannel"),
    ("TimeBetweenUserTypingUpdatesMilliseconds", "ServiceSettings.TimeBetweenUserTypingUpdatesMilliseconds"),
    ("EnableUserTypingMessages", "ServiceSettings.EnableUserTypingMessages"),
    ("EnableChannelViewedMessages", "ServiceSettings.EnableChannelViewedMessages"),
    ("RunJobs", "JobSettings.RunJobs"),
    ("EnableEmailInvitations", "ServiceSettings.EnableEmailInvitations"),
    ("DisableRefetchingOnBrowserFocus", "ExperimentalSettings.DisableRefetchingOnBrowserFocus"),
    ("DisableWakeUpReconnectHandler", "ExperimentalSettings.DisableWakeUpReconnectHandler"),
    (
        "UsersStatusAndProfileFetchingPollIntervalMilliseconds",
        "ExperimentalSettings.UsersStatusAndProfileFetchingPollIntervalMilliseconds",
    ),
    ("EnableCrossTeamSearch", "ServiceSettings.EnableCrossTeamSearch"),
    ("FeatureFlagExperimentalCrossTeamSearch", "ServiceSettings.EnableCrossTeamSearch"),
    ("CustomUrlSchemes", "DisplaySettings.CustomURLSchemes"),
    ("MaxMarkdownNodes", "DisplaySettings.MaxMarkdownNodes"),
    ("CollapsedThreads", "ServiceSettings.CollapsedThreads"),
    ("PostPriority", "ServiceSettings.PostPriority"),
    ("AllowPersistentNotifications", "ServiceSettings.AllowPersistentNotifications"),
    ("AllowPersistentNotificationsForGuests", "ServiceSettings.AllowPersistentNotificationsForGuests"),
    ("PersistentNotificationMaxCount", "ServiceSettings.PersistentNotificationMaxCount"),
    ("PersistentNotificationIntervalMinutes", "ServiceSettings.PersistentNotificationIntervalMinutes"),
    ("PersistentNotificationMaxRecipients", "ServiceSettings.PersistentNotificationMaxRecipients"),
    ("EnableBurnOnRead", "ServiceSettings.EnableBurnOnRead"),
    ("BurnOnReadDurationSeconds", "ServiceSettings.BurnOnReadDurationSeconds"),
    ("BurnOnReadMaximumTimeToLiveSeconds", "ServiceSettings.BurnOnReadMaximumTimeToLiveSeconds"),
    ("AllowSyncedDrafts", "ServiceSettings.AllowSyncedDrafts"),
    ("DelayChannelAutocomplete", "ExperimentalSettings.DelayChannelAutocomplete"),
    ("YoutubeReferrerPolicy", "ExperimentalSettings.YoutubeReferrerPolicy"),
    ("UniqueEmojiReactionLimitPerPost", "ServiceSettings.UniqueEmojiReactionLimitPerPost"),
    ("EnableAttributeBasedAccessControl", "AccessControlSettings.EnableAttributeBasedAccessControl"),
    ("EnableUserManagedAttributes", "AccessControlSettings.EnableUserManagedAttributes"),
    ("EnableAccessControlAuditLogging", "AccessControlSettings.EnableAccessControlAuditLogging"),
    ("EnableChannelPolicyIndicators", "AccessControlSettings.EnableChannelPolicyIndicators"),
    ("WranglerPermittedWranglerRoles", "WranglerSettings.PermittedWranglerRoles"),
    ("WranglerAllowedEmailDomain", "WranglerSettings.AllowedEmailDomain"),
    ("WranglerMoveThreadMaxCount", "WranglerSettings.MoveThreadMaxCount"),
    ("WranglerMoveThreadToAnotherTeamEnable", "WranglerSettings.MoveThreadToAnotherTeamEnable"),
    ("WranglerMoveThreadFromPrivateChannelEnable", "WranglerSettings.MoveThreadFromPrivateChannelEnable"),
    (
        "WranglerMoveThreadFromDirectMessageChannelEnable",
        "WranglerSettings.MoveThreadFromDirectMessageChannelEnable",
    ),
    (
        "WranglerMoveThreadFromGroupMessageChannelEnable",
        "WranglerSettings.MoveThreadFromGroupMessageChannelEnable",
    ),
];

const FULL_FIXED: &[(&str, &str)] = &[
    ("LockProfileFieldsForEmailUsers", "none"),
    ("ExperimentalEnablePostMetadata", "true"),
    ("ExperimentalTimezone", "true"),
    ("CWSMock", ""),
    ("ExperimentalEnableAuthenticationTransfer", "true"),
    ("LdapNicknameAttributeSet", "false"),
    ("LdapFirstNameAttributeSet", "false"),
    ("LdapLastNameAttributeSet", "false"),
    ("LdapPictureAttributeSet", "false"),
    ("LdapPositionAttributeSet", "false"),
    ("EnableCompliance", "false"),
    ("EnableMobileFileDownload", "true"),
    ("EnableMobileFileUpload", "true"),
    ("SamlFirstNameAttributeSet", "false"),
    ("SamlLastNameAttributeSet", "false"),
    ("SamlNicknameAttributeSet", "false"),
    ("SamlPositionAttributeSet", "false"),
    ("EnableCluster", "false"),
    ("EnableMetrics", "false"),
    ("EnableBanner", "false"),
    ("BannerText", ""),
    ("BannerColor", ""),
    ("BannerTextColor", ""),
    ("AllowBannerDismissal", "false"),
    ("EnableThemeSelection", "true"),
    ("DefaultTheme", ""),
    ("AllowCustomThemes", "true"),
    ("AllowedThemes", ""),
    ("DataRetentionEnableMessageDeletion", "false"),
    ("DataRetentionMessageRetentionHours", "0"),
    ("DataRetentionEnableFileDeletion", "false"),
    ("DataRetentionFileRetentionHours", "0"),
    ("ExperimentalSharedChannels", "false"),
    ("EnableCustomGroups", "false"),
    ("AuditLoggingActive", "false"),
];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn env_overrides_by_section_and_type() {
        let mut v = serde_json::json!({"ServiceSettings": {"SiteURL": "", "EnableDeveloper": true, "WebsocketPort": 80}});
        let _ = apply_env(
            &mut v,
            [
                ("MM_SERVICESETTINGS_SITEURL".into(), "https://x".into()),
                ("MM_SERVICESETTINGS_ENABLEDEVELOPER".into(), "false".into()),
                ("MM_SERVICESETTINGS_WEBSOCKETPORT".into(), "443".into()),
            ]
            .into_iter(),
        );
        assert_eq!(v["ServiceSettings"]["SiteURL"], "https://x");
        assert_eq!(v["ServiceSettings"]["EnableDeveloper"], false);
        assert_eq!(v["ServiceSettings"]["WebsocketPort"], 443);
    }
}
