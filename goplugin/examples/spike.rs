//! Runs a plugin bundle against an in-memory API: start, activate, a slash command, HTTP requests, stop. Prints
//! what the plugin asked for that this API does not answer.
//!
//! `cargo run --release -p goplugin --example spike -- <bundle dir> <command> <http path>...`

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};

use anyhow::{Result, anyhow};
use goplugin::gob::{Ty, Value};
use goplugin::host::{self, ApiHandler, Plugin};
use goplugin::rpc::{EMPTY, Reply, empty};

const USER: &str = "uuuuuuuuuuuuuuuuuuuuuuuuuu";
const SITE: &str = "http://localhost:8065";

#[derive(Default)]
struct State {
    dir: String,
    kv: Mutex<BTreeMap<String, Vec<u8>>>,
    config: Mutex<serde_json::Value>,
    missing: Mutex<Vec<String>>,
    commands: Mutex<Vec<String>>,
}

static APP_ERROR: LazyLock<Ty> = LazyLock::new(|| {
    Ty::strukt(
        "AppError",
        &[
            ("Id", Ty::String),
            ("Message", Ty::String),
            ("DetailedError", Ty::String),
            ("RequestId", Ty::String),
            ("StatusCode", Ty::Int),
            ("Where", Ty::String),
        ],
    )
});
static CONFIG: LazyLock<Ty> = LazyLock::new(|| {
    Ty::strukt(
        "Config",
        &[
            (
                "ServiceSettings",
                Ty::strukt("ServiceSettings", &[("SiteURL", Ty::String), ("EnableDeveloper", Ty::Bool)]),
            ),
            ("FileSettings", Ty::strukt("FileSettings", &[("MaxFileSize", Ty::Int)])),
            ("LogSettings", Ty::strukt("LogSettings", &[("EnableDiagnostics", Ty::Bool)])),
        ],
    )
});
static POST: LazyLock<Ty> = LazyLock::new(|| {
    Ty::strukt(
        "Post",
        &[("Id", Ty::String), ("UserId", Ty::String), ("ChannelId", Ty::String), ("Message", Ty::String)],
    )
});
static USER_TY: LazyLock<Ty> = LazyLock::new(|| {
    Ty::strukt("User", &[("Id", Ty::String), ("Username", Ty::String), ("Email", Ty::String), ("Roles", Ty::String)])
});

fn s(v: &str) -> Value {
    Value::String(v.into())
}

fn returns(fields: &[(&str, Ty)], values: Vec<(&str, Value)>) -> Result<(Ty, Value)> {
    Ok((Ty::strukt("Z_Returns", fields), Value::strukt(values)))
}

/// A gob value as JSON, for printing and for the plugin configuration (interfaces unwrapped).
fn json(v: &Value) -> serde_json::Value {
    use serde_json::Value as J;
    match v {
        Value::Bool(b) => J::Bool(*b),
        Value::Int(i) => J::from(*i),
        Value::Uint(u) => J::from(*u),
        Value::Float(f) => J::from(*f),
        Value::String(s) => J::String(s.clone()),
        Value::Bytes(b) | Value::Marshaled(b) => J::String(String::from_utf8_lossy(b).into_owned()),
        Value::List(l) => J::Array(l.iter().map(json).collect()),
        Value::Map(m) => J::Object(m.iter().map(|(k, v)| (k.as_str().unwrap_or("?").to_string(), json(v))).collect()),
        Value::Struct(f) => J::Object(f.iter().map(|(k, v)| (k.clone(), json(v))).collect()),
        Value::Interface(Some(i)) => json(&i.value),
        Value::Interface(None) => J::Null,
        Value::Complex(..) => J::Null,
    }
}

fn api(st: Arc<State>, method: String, a: Value) -> Reply {
    Box::pin(async move {
        let arg = |n: &str| a.get(n).cloned();
        let str_arg = |n: &str| a.get(n).and_then(Value::as_str).unwrap_or("").to_string();
        match method.trim_start_matches("Plugin.") {
            m @ ("LogDebug" | "LogInfo" | "LogWarn" | "LogError") => {
                let kv = arg("B").map(|v| json(&v).to_string()).unwrap_or_default();
                println!("  [{m}] {} {kv}", str_arg("A"));
                Ok((EMPTY.clone(), empty()))
            }
            "GetBundlePath" => returns(&[("A", Ty::String), ("B", Ty::Interface)], vec![("A", s(&st.dir))]),
            "GetServerVersion" => returns(&[("A", Ty::String)], vec![("A", s("12.1.0"))]),
            "GetDiagnosticId" => returns(&[("A", Ty::String)], vec![("A", s("diagnosticid000000000000000"))]),
            "GetLicense" => returns(&[("A", USER_TY.clone())], vec![]),
            "GetConfig" | "GetUnsanitizedConfig" => {
                let cfg = Value::strukt(vec![
                    (
                        "ServiceSettings",
                        Value::strukt(vec![("SiteURL", s(SITE)), ("EnableDeveloper", Value::Bool(false))]),
                    ),
                    ("FileSettings", Value::strukt(vec![("MaxFileSize", Value::Int(100 << 20))])),
                    ("LogSettings", Value::strukt(vec![("EnableDiagnostics", Value::Bool(false))])),
                ]);
                returns(&[("A", CONFIG.clone())], vec![("A", cfg)])
            }
            "LoadPluginConfiguration" => {
                let cfg = st.config.lock().unwrap().clone();
                returns(&[("A", Ty::Bytes)], vec![("A", Value::Bytes(serde_json::to_vec(&cfg)?))])
            }
            "SavePluginConfig" => {
                let cfg = arg("A").map(|v| json(&v)).unwrap_or_default();
                println!("  SavePluginConfig {cfg}");
                *st.config.lock().unwrap() = cfg;
                returns(&[("A", APP_ERROR.clone())], vec![])
            }
            "KVGet" => {
                let v = st.kv.lock().unwrap().get(&str_arg("A")).cloned();
                returns(
                    &[("A", Ty::Bytes), ("B", APP_ERROR.clone())],
                    v.map(|v| vec![("A", Value::Bytes(v))]).unwrap_or_default(),
                )
            }
            "KVSet" => {
                let v = arg("B").and_then(|v| v.as_bytes().map(<[u8]>::to_vec)).unwrap_or_default();
                st.kv.lock().unwrap().insert(str_arg("A"), v);
                returns(&[("A", APP_ERROR.clone())], vec![])
            }
            "KVSetWithOptions" => {
                let key = str_arg("A");
                let new = arg("B").and_then(|v| v.as_bytes().map(<[u8]>::to_vec));
                let opts = arg("C").unwrap_or(empty());
                let atomic = opts.get("Atomic").and_then(Value::as_bool).unwrap_or(false);
                let old = opts.get("OldValue").and_then(|v| v.as_bytes().map(<[u8]>::to_vec));
                let mut kv = st.kv.lock().unwrap();
                let ok = !atomic || kv.get(&key).cloned() == old;
                if ok {
                    match new {
                        Some(v) => kv.insert(key, v),
                        None => kv.remove(&key),
                    };
                }
                returns(&[("A", Ty::Bool), ("B", APP_ERROR.clone())], vec![("A", Value::Bool(ok))])
            }
            "KVDelete" => {
                st.kv.lock().unwrap().remove(&str_arg("A"));
                returns(&[("A", APP_ERROR.clone())], vec![])
            }
            "KVList" => {
                let keys = st.kv.lock().unwrap().keys().map(|k| s(k)).collect();
                returns(&[("A", Ty::slice(Ty::String)), ("B", APP_ERROR.clone())], vec![("A", Value::List(keys))])
            }
            "EnsureBotUser" => {
                println!("  EnsureBotUser {}", json(&arg("A").unwrap_or(empty())));
                returns(&[("A", Ty::String), ("B", Ty::Interface)], vec![("A", s("botbotbotbotbotbotbotbotbo"))])
            }
            "SetProfileImage" => returns(&[("A", APP_ERROR.clone())], vec![]),
            "UnregisterCommand" => returns(&[("A", Ty::Interface)], vec![]),
            "SendEphemeralPost" => {
                let p = arg("B").unwrap_or(empty());
                let field = |n: &str| p.get(n).and_then(Value::as_str).unwrap_or("").to_string();
                println!(
                    "  SendEphemeralPost to {}: {}",
                    str_arg("A"),
                    field("Message").chars().take(400).collect::<String>()
                );
                let post = Value::strukt(vec![
                    ("Id", s("eeeeeeeeeeeeeeeeeeeeeeeeee")),
                    ("ChannelId", s(&field("ChannelId"))),
                    ("UserId", s(&field("UserId"))),
                    ("Message", s(&field("Message"))),
                ]);
                returns(&[("A", POST.clone())], vec![("A", post)])
            }
            "RegisterCommand" => {
                let c = arg("A").unwrap_or(empty());
                let trigger = c.get("Trigger").and_then(Value::as_str).unwrap_or("").to_string();
                println!("  RegisterCommand /{trigger}");
                st.commands.lock().unwrap().push(trigger);
                returns(&[("A", Ty::Interface)], vec![])
            }
            "GetUser" => {
                let u = Value::strukt(vec![
                    ("Id", s(&str_arg("A"))),
                    ("Username", s("alice")),
                    ("Email", s("alice@example.com")),
                    ("Roles", s("system_user")),
                ]);
                returns(&[("A", USER_TY.clone()), ("B", APP_ERROR.clone())], vec![("A", u)])
            }
            m => {
                st.missing.lock().unwrap().push(m.to_string());
                Err(anyhow!("{m} is not implemented"))
            }
        }
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().ok_or_else(|| anyhow!("bundle dir"))?);
    let command = args.next().unwrap_or_default();
    let paths: Vec<String> = args.collect();
    let exe = dir.join("server/dist/plugin-linux-amd64");
    let st = Arc::new(State { dir: dir.to_string_lossy().into(), ..Default::default() });
    let t = std::time::Instant::now();
    let p = Plugin::start(&exe, &dir, Arc::new(|l: &str| println!("  [stdout/stderr] {l}"))).await?;
    println!("started in {:?}; implements {:?}", t.elapsed(), p.implemented);
    let handler: ApiHandler = {
        let st = st.clone();
        Arc::new(move |m, a| api(st.clone(), m, a))
    };
    let t = std::time::Instant::now();
    match p.activate(handler).await {
        Ok(()) => println!("activated in {:?}", t.elapsed()),
        Err(e) => println!("activation failed after {:?}: {e:#}", t.elapsed()),
    }
    let ctx = Value::strukt(vec![("RequestId", s("req1")), ("SessionId", s("sess1"))]);
    if !command.is_empty() {
        let cargs = Value::strukt(vec![
            ("UserId", s(USER)),
            ("ChannelId", s("cccccccccccccccccccccccccc")),
            ("TeamId", s("tttttttttttttttttttttttttt")),
            ("Command", s(&command)),
            ("SiteURL", s(SITE)),
        ]);
        match p.execute_command(ctx.clone(), cargs).await {
            Ok((resp, err)) => println!(
                "ExecuteCommand {command:?}: response {} error {}",
                resp.map(|v| json(&v).to_string()).unwrap_or_default(),
                err.map(|v| json(&v).to_string()).unwrap_or_default()
            ),
            Err(e) => println!("ExecuteCommand failed: {e:#}"),
        }
    }
    for path in paths {
        let (method, path, body) = match path.split_once(' ') {
            Some((p, b)) => ("POST", p.to_string(), b.as_bytes().to_vec()),
            None => ("GET", path, vec![]),
        };
        let req = Value::strukt(vec![
            ("Method", s(method)),
            ("URL", Value::Marshaled(path.as_bytes().to_vec())),
            ("Proto", s("HTTP/1.1")),
            ("ProtoMajor", Value::Int(1)),
            ("ProtoMinor", Value::Int(1)),
            (
                "Header",
                host::header_value(&[
                    ("Mattermost-User-Id".into(), vec![USER.into()]),
                    ("Content-Type".into(), vec!["application/json".into()]),
                ]),
            ),
            ("Host", s("localhost:8065")),
            ("RemoteAddr", s("127.0.0.1:50000")),
            ("RequestURI", s(&path)),
        ]);
        match p.serve_http(ctx.clone(), req, body).await {
            Ok(r) => println!(
                "ServeHTTP {method} {path}: {} {:?} {}",
                r.status,
                r.headers,
                String::from_utf8_lossy(&r.body).chars().take(300).collect::<String>()
            ),
            Err(e) => println!("ServeHTTP {path} failed: {e:#}"),
        }
    }
    println!("commands registered: {:?}", st.commands.lock().unwrap());
    println!("API methods not implemented here: {:?}", st.missing.lock().unwrap());
    let t = std::time::Instant::now();
    p.stop().await;
    println!("stopped in {:?}", t.elapsed());
    Ok(())
}
