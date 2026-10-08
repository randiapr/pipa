//! Mapping between the domain types (`crate::datasource`/`crate::project`) and the wire types of
//! the `pipa-api` contract crate. This is the only place the two shapes meet, so the domain stays
//! free of wire concerns and the wire format stays free of domain ones.

use crate::datasource::domain::{
    ConnectionConfig, ConnectionTestOutcome, DataSource, DataSourceError, DbEngine, NewDataSource,
};
use crate::project::{NewProject, Project, ProjectId, ProjectUpdate};
use crate::user::{
    NewUser as NewUserAccount, Role, User, UserError, UserUpdate as UserAccountUpdate,
};
use uuid::Uuid;

impl From<pipa_api::DbEngine> for DbEngine {
    fn from(engine: pipa_api::DbEngine) -> Self {
        match engine {
            pipa_api::DbEngine::Postgres => Self::Postgres,
            pipa_api::DbEngine::MySql => Self::MySql,
        }
    }
}

impl From<DbEngine> for pipa_api::DbEngine {
    fn from(engine: DbEngine) -> Self {
        match engine {
            DbEngine::Postgres => Self::Postgres,
            DbEngine::MySql => Self::MySql,
        }
    }
}

impl From<pipa_api::ConnectionConfig> for ConnectionConfig {
    fn from(config: pipa_api::ConnectionConfig) -> Self {
        Self {
            host: config.host,
            port: config.port,
            username: config.username,
            password: config.password,
            database: config.database,
        }
    }
}

impl From<ConnectionConfig> for pipa_api::ConnectionConfig {
    fn from(config: ConnectionConfig) -> Self {
        Self {
            host: config.host,
            port: config.port,
            username: config.username,
            password: config.password,
            database: config.database,
        }
    }
}

impl From<ConnectionTestOutcome> for pipa_api::ConnectionTestOutcome {
    fn from(outcome: ConnectionTestOutcome) -> Self {
        match outcome {
            ConnectionTestOutcome::Reachable => Self::Reachable,
            ConnectionTestOutcome::Unreachable { reason } => Self::Unreachable { reason },
        }
    }
}

impl From<DataSource> for pipa_api::DataSourceView {
    fn from(source: DataSource) -> Self {
        Self {
            id: source.id.to_string(),
            name: source.name,
            engine: source.engine.into(),
            connection: source.connection.into(),
            project_id: source.project_id.map(|id| id.to_string()),
            registered_at_unix: source.registered_at_unix,
        }
    }
}

/// Fails with `InvalidField` if `project_id` isn't a UUID — the wire carries it as a string.
impl TryFrom<pipa_api::NewDataSource> for NewDataSource {
    type Error = DataSourceError;

    fn try_from(new: pipa_api::NewDataSource) -> Result<Self, Self::Error> {
        let project_id = new
            .project_id
            .map(|id| {
                Uuid::parse_str(&id).map(ProjectId).map_err(|_| {
                    DataSourceError::InvalidField("project_id must be a valid UUID".to_string())
                })
            })
            .transpose()?;
        Ok(Self {
            name: new.name,
            engine: new.engine.into(),
            connection: new.connection.into(),
            project_id,
        })
    }
}

impl From<Project> for pipa_api::ProjectView {
    fn from(project: Project) -> Self {
        Self {
            id: project.id.to_string(),
            name: project.name,
            description: project.description,
            created_at_unix: project.created_at_unix,
        }
    }
}

impl From<pipa_api::NewProject> for NewProject {
    fn from(new: pipa_api::NewProject) -> Self {
        Self {
            name: new.name,
            description: new.description,
        }
    }
}

impl From<pipa_api::ProjectUpdate> for ProjectUpdate {
    fn from(update: pipa_api::ProjectUpdate) -> Self {
        Self {
            name: update.name,
            description: update.description,
        }
    }
}

impl From<pipa_api::Role> for Role {
    fn from(role: pipa_api::Role) -> Self {
        match role {
            pipa_api::Role::Admin => Self::Admin,
            pipa_api::Role::Developer => Self::Developer,
            pipa_api::Role::User => Self::User,
        }
    }
}

impl From<Role> for pipa_api::Role {
    fn from(role: Role) -> Self {
        match role {
            Role::Admin => Self::Admin,
            Role::Developer => Self::Developer,
            Role::User => Self::User,
        }
    }
}

impl From<User> for pipa_api::UserView {
    fn from(user: User) -> Self {
        Self {
            id: user.id.to_string(),
            username: user.username,
            role: user.role.into(),
            project_ids: user.project_ids.iter().map(ToString::to_string).collect(),
            created_at_unix: user.created_at_unix,
        }
    }
}

/// Fails with `InvalidField` if any project id isn't a UUID — the wire carries them as strings.
fn parse_project_ids(ids: Vec<String>) -> Result<Vec<ProjectId>, UserError> {
    ids.iter()
        .map(|id| {
            Uuid::parse_str(id)
                .map(ProjectId)
                .map_err(|_| UserError::InvalidField("project_ids must be valid UUIDs".to_string()))
        })
        .collect()
}

impl TryFrom<pipa_api::NewUser> for NewUserAccount {
    type Error = UserError;

    fn try_from(new: pipa_api::NewUser) -> Result<Self, Self::Error> {
        Ok(Self {
            username: new.username,
            password: new.password,
            role: new.role.into(),
            project_ids: parse_project_ids(new.project_ids)?,
        })
    }
}

impl TryFrom<pipa_api::UserUpdate> for UserAccountUpdate {
    type Error = UserError;

    fn try_from(update: pipa_api::UserUpdate) -> Result<Self, Self::Error> {
        Ok(Self {
            role: update.role.map(Into::into),
            password: update.password,
            project_ids: update.project_ids.map(parse_project_ids).transpose()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn developer_role_round_trips() {
        assert_eq!(Role::from(pipa_api::Role::Developer), Role::Developer);
        assert_eq!(
            pipa_api::Role::from(Role::Developer),
            pipa_api::Role::Developer
        );
    }

    #[test]
    fn new_user_parses_project_ids_and_role() {
        let id = Uuid::now_v7();
        let new = NewUserAccount::try_from(pipa_api::NewUser {
            username: "alice".to_string(),
            password: "correct horse".to_string(),
            role: pipa_api::Role::User,
            project_ids: vec![id.to_string()],
        })
        .unwrap();
        assert_eq!(new.role, Role::User);
        assert_eq!(new.project_ids, vec![ProjectId(id)]);
    }

    #[test]
    fn new_user_rejects_malformed_project_ids() {
        let err = NewUserAccount::try_from(pipa_api::NewUser {
            username: "alice".to_string(),
            password: "correct horse".to_string(),
            role: pipa_api::Role::User,
            project_ids: vec!["nope".to_string()],
        })
        .unwrap_err();
        assert!(matches!(err, UserError::InvalidField(_)));
    }

    #[test]
    fn user_view_never_carries_the_password_hash() {
        let user = User::register(
            NewUserAccount {
                username: "alice".to_string(),
                password: "correct horse".to_string(),
                role: Role::Admin,
                project_ids: Vec::new(),
            },
            "secret-hash".to_string(),
        );
        let json = serde_json::to_string(&pipa_api::UserView::from(user)).unwrap();
        assert!(!json.contains("secret-hash"));
        assert!(json.contains("\"role\":\"admin\""));
    }

    fn new_source(project_id: Option<&str>) -> pipa_api::NewDataSource {
        pipa_api::NewDataSource {
            name: "orders".to_string(),
            engine: pipa_api::DbEngine::MySql,
            connection: pipa_api::ConnectionConfig {
                host: "db".to_string(),
                port: 3306,
                username: "u".to_string(),
                password: "p".to_string(),
                database: "d".to_string(),
            },
            project_id: project_id.map(str::to_string),
        }
    }

    #[test]
    fn new_data_source_parses_project_id() {
        let id = Uuid::now_v7();
        let new = NewDataSource::try_from(new_source(Some(&id.to_string()))).unwrap();
        assert_eq!(new.project_id, Some(ProjectId(id)));
        assert_eq!(new.engine, DbEngine::MySql);
    }

    #[test]
    fn new_data_source_rejects_malformed_project_id() {
        let err = NewDataSource::try_from(new_source(Some("nope"))).unwrap_err();
        assert!(matches!(err, DataSourceError::InvalidField(_)));
    }

    #[test]
    fn data_source_maps_to_view() {
        let source = DataSource::register(new_source(None).try_into().unwrap()).unwrap();
        let id = source.id.to_string();
        let view = pipa_api::DataSourceView::from(source);
        assert_eq!(view.id, id);
        assert_eq!(view.engine, pipa_api::DbEngine::MySql);
        assert_eq!(view.project_id, None);
    }
}
