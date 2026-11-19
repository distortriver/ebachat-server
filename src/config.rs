//! Mattermost's `config.json` (or the active row of `Configurations`), with `MM_SECTION_KEY` environment
//! overrides, and the client config the API sends from it.

// Based on Mattermost server/config/client.go and config/environment.go, Copyright (c) 2015-present
// Mattermost, Inc. Licensed under AGPL-3.0.

use std::collections::BTreeMap;

use serde_json::Value;

pub struct Config {
    pub v: Value,
    pub diagnostic_id: String,
}

impl Config {
    /// `MM_CONFIG`: a file (default `config/config.json`) or a Postgres DSN for the `Configurations` table.
    pub async fn load() -> anyhow::Result<Config> {
        let src = std::env::var("MM_CONFIG").unwrap_or_else(|_| "config/config.json".into());
        let mut v: Value = if src.starts_with("postgres") {
            let db = sqlx::PgPool::connect(&crate::db_url(&src)).await?;
            let (s,): (String,) = sqlx::query_as("SELECT Value FROM Configurations WHERE Active")
                .fetch_one(&db)
                .await?;
            serde_json::from_str(&s)?
        } else {
            serde_json::from_slice(&std::fs::read(&src).map_err(|e| anyhow::anyhow!("{src}: {e}"))?)?
        };
        apply_env(&mut v, std::env::vars());
        Ok(Config { v, diagnostic_id: String::new() })
    }

    pub fn get(&self, path: &str) -> &Value {
        let mut v = &self.v;
        for k in path.split('.') {
            v = v.get(k).unwrap_or(&Value::Null);
        }
        v
    }
    pub fn str(&self, path: &str) -> &str {
        self.get(path).as_str().unwrap_or("")
    }
    pub fn bool(&self, path: &str) -> bool {
        self.get(path).as_bool().unwrap_or(false)
    }
    pub fn i64(&self, path: &str) -> i64 {
        self.get(path).as_i64().unwrap_or(0)
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
            ("BuildHash", ""),
            ("BuildHashEnterprise", "none"),
            ("BuildEnterpriseReady", "false"),
            ("ServiceEnvironment", "production"),
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
        put("WebsocketURL", self.str("ServiceSettings.WebsocketURL").trim_end_matches('/').into());
        put("DiagnosticId", self.diagnostic_id.clone());
        put("TelemetryId", self.diagnostic_id.clone());
        put("AppsPluginEnabled", "false".into());
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
        let giphy = self.str("ServiceSettings.GiphySdkKey");
        put("GiphySdkKey", giphy.into());
        put("IsDefaultMarketplace", "true".into());
        p
    }
}

/// `MM_SERVICESETTINGS_SITEURL=...`: a setting by its section and name, case-insensitive, typed as the value
/// it replaces.
fn apply_env(v: &mut Value, vars: impl Iterator<Item = (String, String)>) {
    for (k, val) in vars {
        let Some(rest) = k.strip_prefix("MM_") else { continue };
        set_path(v, rest, &val);
    }
}

fn set_path(v: &mut Value, key: &str, val: &str) {
    let Some(m) = v.as_object_mut() else { return };
    // Section names have no underscores; walk down matching `SECTION_` prefixes.
    let names: Vec<String> = m.keys().cloned().collect();
    for name in names {
        let up = name.to_uppercase();
        if key == up {
            let old = &m[&name];
            m.insert(name.clone(), typed(old, val));
            return;
        }
        if let Some(rest) = key.strip_prefix(&format!("{up}_"))
            && m[&name].is_object()
        {
            set_path(m.get_mut(&name).unwrap(), rest, val);
            return;
        }
    }
}

fn typed(old: &Value, s: &str) -> Value {
    match old {
        Value::Bool(_) => Value::Bool(s == "true"),
        Value::Number(_) => s.parse::<i64>().map(Value::from).unwrap_or_else(|_| Value::String(s.into())),
        Value::Array(_) => Value::Array(s.split(' ').map(|x| Value::String(x.into())).collect()),
        _ => Value::String(s.into()),
    }
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
    ("CWSURL", ""),
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
    ("CWSMock", "false"),
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
        apply_env(
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
