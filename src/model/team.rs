// Based on Mattermost server/public/model/team.go and team_member.go, Copyright (c) 2015-present Mattermost,
// Inc. Licensed under AGPL-3.0.

use ebachat_macros::model;

#[model(table = "teams")]
#[derive(Debug, Clone, PartialEq)]
pub struct Team {
    pub id: String,
    pub create_at: i64,
    pub update_at: i64,
    pub delete_at: i64,
    pub display_name: String,
    pub name: String,
    pub description: String,
    pub email: String,
    #[mm(pgtype = "team_type")]
    pub r#type: String,
    pub company_name: String,
    pub allowed_domains: String,
    pub invite_id: String,
    pub allow_open_invite: bool,
    #[mm(omitempty)]
    pub last_team_icon_update: i64,
    pub scheme_id: Option<String>,
    pub group_constrained: Option<bool>,
    #[mm(transient)]
    pub policy_id: Option<String>,
    pub cloud_limits_archived: bool,
    #[mm(transient)]
    pub policy_enforced: bool,
    #[mm(transient)]
    pub policy_auto_add: bool,
    #[mm(transient)]
    pub policy_is_active: bool,
}

impl Team {
    /// `Team.Sanitize`: what someone who may not manage the team does not see.
    pub fn sanitize(&mut self) {
        self.email.clear();
        self.invite_id.clear();
    }
}

#[model(table = "teammembers")]
#[derive(Debug, Clone, PartialEq)]
pub struct TeamMember {
    pub team_id: String,
    pub user_id: String,
    pub roles: String,
    pub delete_at: i64,
    pub scheme_guest: bool,
    pub scheme_user: bool,
    pub scheme_admin: bool,
    #[mm(transient)]
    pub explicit_roles: String,
    #[mm(skip_json)]
    pub create_at: i64,
}
