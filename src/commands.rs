//! Slash commands: the built-in triggers' suggestions, and the built-in commands that are statuses or posts
//! (`/away`, `/online`, `/offline`, `/dnd`, `/me`, `/shrug`, `/code`, `/echo`). The other built-ins (channel and
//! team management) and custom commands answer as Go does for an unknown trigger.

// Based on Mattermost server/channels/api4/command.go, app/command.go (ExecuteCommand, HandleCommandResponse)
// and app/slashcommands, Copyright (c) 2015-present Mattermost, Inc. Licensed under AGPL-3.0.

use std::sync::OnceLock;

use axum::extract::{Path, Query, State as St};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::app::{ApiResult, AppError, Auth, Body, State, is_valid_id, new_id, now_ms};
use crate::hub::Broadcast;
use crate::model::{Post, Session, channel};
use crate::perm;

pub fn routes() -> Router<State> {
    Router::new()
        .route("/api/v4/commands/execute", post(execute))
        .route("/api/v4/teams/{team_id}/commands/autocomplete_suggestions", get(suggestions))
}

#[derive(Deserialize)]
struct Builtin {
    trigger: String,
    hint: String,
    description: String,
}

fn builtins() -> &'static [Builtin] {
    static B: OnceLock<Vec<Builtin>> = OnceLock::new();
    B.get_or_init(|| serde_json::from_str(include_str!("../data/commands.json")).unwrap_or_default())
}

/// `model.AutocompleteSuggestion`.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct Suggestion {
    complete: String,
    suggestion: String,
    hint: String,
    description: String,
    icon_data: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SuggestionQuery {
    user_input: String,
}

/// `listCommandAutocompleteSuggestions`: the triggers that start with what was typed (arguments of built-ins
/// are not suggested).
async fn suggestions(
    St(app): St<State>,
    Auth(s): Auth,
    Path(team_id): Path<String>,
    Query(q): Query<SuggestionQuery>,
) -> ApiResult<Json<Vec<Suggestion>>> {
    if !is_valid_id(&team_id) {
        return Err(AppError::invalid_url_param("team_id"));
    }
    app.require_team(&s, &team_id, perm::VIEW_TEAM).await?;
    if q.user_input.is_empty() {
        return Err(AppError::invalid_param("userInput"));
    }
    let input = q.user_input.trim_start_matches('/');
    if input.contains(' ') {
        return Ok(Json(vec![]));
    }
    Ok(Json(
        builtins()
            .iter()
            .filter(|b| b.trigger.starts_with(input))
            .map(|b| Suggestion {
                complete: b.trigger.clone(),
                suggestion: b.trigger.clone(),
                hint: b.hint.clone(),
                description: b.description.clone(),
                icon_data: String::new(),
            })
            .collect(),
    ))
}

/// `model.CommandArgs` as the client sends it.
#[derive(Deserialize, Default)]
#[serde(default)]
struct CommandArgs {
    channel_id: String,
    team_id: String,
    root_id: String,
    command: String,
}

/// `model.CommandResponse`.
#[derive(Serialize, Default)]
struct CommandResponse {
    response_type: String,
    text: String,
    username: String,
    channel_id: String,
    icon_url: String,
    #[serde(rename = "type")]
    kind: String,
    props: Option<Value>,
    goto_location: String,
    trigger_id: String,
    skip_slack_parsing: bool,
    attachments: Option<Value>,
    extra_responses: Option<Value>,
}

fn ephemeral(text: &str) -> CommandResponse {
    CommandResponse { response_type: "ephemeral".into(), text: text.into(), ..Default::default() }
}

fn tr(id: &str) -> String {
    crate::i18n::t(id).unwrap_or(id).to_string()
}

/// `executeCommand`.
async fn execute(St(app): St<State>, Auth(s): Auth, Body(mut args): Body<CommandArgs>) -> ApiResult<Json<CommandResponse>> {
    if args.command.len() <= 1 || !args.command.starts_with('/') || !is_valid_id(&args.channel_id) {
        return Err(AppError::bad_request("api.command.execute_command.start.app_error", ""));
    }
    if !app.channel_perm(&s, &args.channel_id, perm::CREATE_POST).await?.0 {
        return Err(AppError::forbidden_perm(perm::CREATE_POST));
    }
    let ch = app.channel_or_404(&args.channel_id).await?;
    if ch.delete_at != 0 {
        return Err(AppError::bad_request("api.command.execute_command.deleted.error", ""));
    }
    if ch.r#type != channel::DIRECT && ch.r#type != channel::GROUP {
        args.team_id = ch.team_id.clone();
    }
    let (trigger, message) = match args.command[1..].split_once(' ') {
        Some((t, m)) => (t.to_lowercase(), m.trim().to_string()),
        None => (args.command[1..].to_lowercase(), String::new()),
    };
    let response = match trigger.as_str() {
        "away" => {
            app.set_status_away_if_needed(&s.user_id, true).await;
            ephemeral(&tr("api.command_away.success"))
        }
        "online" => {
            app.set_status_online(&s.user_id, true).await;
            ephemeral(&tr("api.command_online.success"))
        }
        "offline" => {
            app.set_status_offline(&s.user_id, true, false).await;
            ephemeral(&tr("api.command_offline.success"))
        }
        "dnd" => {
            app.set_status_dnd(&s.user_id, None).await;
            ephemeral(&tr("api.command_dnd.success"))
        }
        "me" => CommandResponse {
            response_type: "in_channel".into(),
            kind: "me".into(),
            text: format!("*{message}*"),
            ..Default::default()
        },
        "shrug" => {
            let shrug = r"¯\\\_(ツ)\_/¯";
            let text = if message.is_empty() { shrug.to_string() } else { format!("{message} {shrug}") };
            CommandResponse { response_type: "in_channel".into(), text, ..Default::default() }
        }
        "code" if message.is_empty() => ephemeral(&tr("api.command_code.message.app_error")),
        "code" => CommandResponse {
            response_type: "in_channel".into(),
            text: format!("    {}", message.split('\n').collect::<Vec<_>>().join("\n    ")),
            skip_slack_parsing: true,
            ..Default::default()
        },
        "echo" if message.is_empty() => ephemeral(&tr("api.command_echo.message.app_error")),
        "echo" => CommandResponse { response_type: "in_channel".into(), text: echo_text(&message), ..Default::default() },
        _ => {
            let mut e = AppError::not_found("api.command.execute_command.not_found.app_error", "");
            if let Some(m) = crate::i18n::tr(&e.id, &[("Trigger", &trigger)]) {
                e.message = m;
            }
            return Err(e);
        }
    };
    respond(&app, &s, &args, &response, &trigger).await?;
    Ok(Json(response))
}

/// `/echo "text" [delay]`: the quoted text (the delay is not kept).
fn echo_text(message: &str) -> String {
    let m = message.trim();
    if let Some(rest) = m.strip_prefix('"')
        && let Some(end) = rest.rfind('"')
    {
        return rest[..end].to_string();
    }
    m.to_string()
}

/// `HandleCommandResponsePost` / `CreateCommandPost`: in the channel as a post, or to the user only.
async fn respond(app: &State, s: &Session, args: &CommandArgs, r: &CommandResponse, trigger: &str) -> ApiResult<()> {
    let post = Post {
        channel_id: args.channel_id.clone(),
        root_id: args.root_id.clone(),
        user_id: s.user_id.clone(),
        r#type: r.kind.clone(),
        message: r.text.clone(),
        ..Default::default()
    };
    if r.response_type == "in_channel" {
        if let Err(e) = app.create_post(post, true).await {
            eprintln!("command /{trigger}: {}", e.detailed_error);
            let mut e = AppError::new(StatusCode::INTERNAL_SERVER_ERROR, "api.command.execute_command.create_post_failed.app_error", "");
            if let Some(m) = crate::i18n::tr(&e.id, &[("Trigger", trigger)]) {
                e.message = m;
            }
            return Err(e);
        }
        return Ok(());
    }
    if r.text.is_empty() {
        return Ok(());
    }
    // SendEphemeralPost
    let mut post = post;
    post.id = new_id();
    post.create_at = now_ms();
    post.r#type = "system_ephemeral".into();
    let data = json!({ "post": serde_json::to_string(&post).unwrap() });
    app.hub.publish(
        "ephemeral_message",
        data,
        Broadcast { channel_id: args.channel_id.clone(), user_id: s.user_id.clone(), ..Default::default() },
    );
    Ok(())
}
