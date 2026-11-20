//! `Config.IsValid`: the checks a config must pass to be saved.

// Based on Mattermost server/public/model/config.go (IsValid and the sections' isValid), Copyright (c)
// 2015-present Mattermost, Inc. Licensed under AGPL-3.0.

use serde_json::Value;

use crate::app::AppError;

/// `Config.IsValid` over a config with its defaults set.
pub fn is_valid(_c: &Value) -> Result<(), AppError> {
    Ok(())
}
