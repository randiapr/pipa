//! What most tests start from: a backend of their own on a fresh bucket with the first admin
//! signed in, plus fixtures to set up projects, accounts and data sources through its API.

use std::sync::{Arc, LazyLock};

use pipa_api::{
    DataSourceData, DataSourceView, IngestedTablesUpdate, NewDataSource, NewProject, NewUser,
    ProjectData, ProjectView, ReadTableRequest, Role, SourceTableRef, UserData, UserView, path,
};
use serde_json::Value;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

use crate::backend::{self, ADMIN_PASSWORD, ADMIN_USERNAME, Backend, BackendConfig};
use crate::infra;
use crate::ingestion::{Ingestion, IngestionConfig};
use crate::postgres::Database;
use crate::reply::Reply;
use crate::wait;

/// Password of every account [`TestEnv::account`] creates.
pub const ACCOUNT_PASSWORD: &str = "account-password-1";

/// How many [`TestEnv`]s may exist at once: each is a backend (and usually ingestion) process, and
/// more than a few only make the tests compete for the one Docker daemon and the cores.
const CONCURRENT_ENVS: usize = 6;

pub struct TestEnv {
    _permit: OwnedSemaphorePermit,
    pub backend: Backend,
    pub bucket: String,
    /// Bearer token of the first admin.
    pub admin: String,
}

/// An account created through the API, with a token for it.
pub struct Account {
    pub user: UserView,
    pub token: String,
}

impl TestEnv {
    pub async fn new() -> Self {
        Self::with(|_| {}).await
    }

    /// Like [`Self::new`], with the backend's configuration adjusted first.
    pub async fn with(configure: impl FnOnce(&mut BackendConfig)) -> Self {
        static SLOTS: LazyLock<Arc<Semaphore>> =
            LazyLock::new(|| Arc::new(Semaphore::new(CONCURRENT_ENVS)));
        let permit = Arc::clone(&SLOTS)
            .acquire_owned()
            .await
            .expect("the semaphore is never closed");
        let bucket = new_bucket().await;
        let mut config = BackendConfig::new(&bucket);
        configure(&mut config);
        let backend = Backend::start(config).await.expect("starting pipa-backend");
        let admin = backend.token(ADMIN_USERNAME, ADMIN_PASSWORD).await;
        backend::wait_for_catalog(&backend, &admin).await;
        Self {
            _permit: permit,
            backend,
            bucket,
            admin,
        }
    }

    // ---- fixtures: each asserts that the request succeeded ----

    pub async fn project(&self, name: &str) -> ProjectView {
        self.backend
            .post(path::PROJECTS)
            .bearer(&self.admin)
            .json(&NewProject {
                name: name.to_string(),
                description: None,
            })
            .send()
            .await
            .created()
            .parse::<ProjectData>()
            .project
    }

    /// A project with a name no other has.
    pub async fn unique_project(&self) -> ProjectView {
        self.project(&format!("project-{}", Uuid::now_v7().simple()))
            .await
    }

    pub async fn account(&self, username: &str, role: Role, projects: &[&ProjectView]) -> Account {
        let user = self
            .backend
            .post(path::USERS)
            .bearer(&self.admin)
            .json(&NewUser {
                username: username.to_string(),
                password: ACCOUNT_PASSWORD.to_string(),
                role,
                project_ids: projects.iter().map(|project| project.id.clone()).collect(),
            })
            .send()
            .await
            .created()
            .parse::<UserData>()
            .user;
        let token = self.backend.token(username, ACCOUNT_PASSWORD).await;
        Account { user, token }
    }

    /// An account with a unique username.
    pub async fn developer(&self, projects: &[&ProjectView]) -> Account {
        self.account(&unique("dev"), Role::Developer, projects)
            .await
    }

    /// A view-only account with a unique username.
    pub async fn viewer(&self, projects: &[&ProjectView]) -> Account {
        self.account(&unique("viewer"), Role::User, projects).await
    }

    pub async fn source(
        &self,
        project: Option<&ProjectView>,
        name: &str,
        database: &Database,
    ) -> DataSourceView {
        self.backend
            .post(path::DATASOURCES)
            .bearer(&self.admin)
            .json(&new_source(project, name, database))
            .send()
            .await
            .created()
            .parse::<DataSourceData>()
            .datasource
    }

    /// Chooses the tables of `source` to ingest, replacing the previous choice.
    pub async fn ingest(&self, source: &DataSourceView, tables: &[(&str, &str)]) {
        self.backend
            .put(&path::datasource_tables(&source.id))
            .bearer(&self.admin)
            .json(&IngestedTablesUpdate {
                tables: tables
                    .iter()
                    .map(|(schema, name)| SourceTableRef {
                        schema: schema.to_string(),
                        name: name.to_string(),
                    })
                    .collect(),
            })
            .send()
            .await
            .ok();
    }

    pub async fn ingestion(&self) -> Ingestion {
        self.ingestion_with(|_| {}).await
    }

    /// An ingestion instance on this environment's bucket, configured by `configure`.
    pub async fn ingestion_with(&self, configure: impl FnOnce(&mut IngestionConfig)) -> Ingestion {
        let mut config = IngestionConfig::new(&self.bucket);
        configure(&mut config);
        Ingestion::start(config)
            .await
            .expect("starting pipa-ingestion")
    }

    // ---- reading the ingested data ----

    /// `POST /tables/rows` as `token`.
    pub async fn table_rows(
        &self,
        token: &str,
        project: &ProjectView,
        source: &DataSourceView,
        table: &str,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Reply {
        self.backend
            .post(path::TABLE_ROWS)
            .bearer(token)
            .json(&ReadTableRequest {
                project_id: project.id.clone(),
                source_id: source.id.clone(),
                table: table.to_string(),
                limit,
                offset,
            })
            .send()
            .await
    }

    /// `POST /query` as the admin, scoped to `project`.
    pub async fn query(&self, project: &ProjectView, sql: &str) -> Reply {
        self.backend
            .post(path::QUERY)
            .bearer(&self.admin)
            .json(&pipa_api::QueryRequest {
                sql: sql.to_string(),
                project_id: Some(project.id.clone()),
            })
            .send()
            .await
    }

    /// Waits until the current rows of `table` satisfy `done`, and returns them.
    pub async fn wait_for_rows(
        &self,
        project: &ProjectView,
        source: &DataSourceView,
        table: &str,
        what: &str,
        done: impl Fn(&[Value]) -> bool,
    ) -> Vec<Value> {
        wait::eventually(what, || async {
            let reply = self
                .table_rows(&self.admin, project, source, table, Some(1000), None)
                .await;
            if reply.status != 200 {
                return Err(reply.body.to_string());
            }
            let rows = reply.rows();
            if done(rows) {
                Ok(rows.clone())
            } else {
                Err(format!("{rows:?}"))
            }
        })
        .await
    }

    /// Waits until the changelog table of `table` (`POST /query`) satisfies `done`.
    pub async fn wait_for_query(
        &self,
        project: &ProjectView,
        sql: &str,
        what: &str,
        done: impl Fn(&[Value]) -> bool,
    ) -> Vec<Value> {
        wait::eventually(what, || async {
            let reply = self.query(project, sql).await;
            if reply.status != 200 {
                return Err(reply.body.to_string());
            }
            let rows = reply.rows();
            if done(rows) {
                Ok(rows.clone())
            } else {
                Err(format!("{rows:?}"))
            }
        })
        .await
    }
}

pub fn new_source(project: Option<&ProjectView>, name: &str, database: &Database) -> NewDataSource {
    NewDataSource {
        name: name.to_string(),
        engine: pipa_api::DbEngine::Postgres,
        connection: database.connection(),
        project_id: project.map(|project| project.id.clone()),
    }
}

/// `prefix` plus a random suffix: usernames and project names are unique per backend, but tests
/// shouldn't depend on what other fixtures created.
pub fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::now_v7().simple().to_string()[20..])
}

/// Creates a bucket of its own for a test.
pub async fn new_bucket() -> String {
    let name = format!("t-{}", &Uuid::now_v7().simple().to_string()[10..]);
    infra::create_bucket(&name)
        .await
        .expect("creating a bucket");
    name
}

/// The Iceberg namespace `pipa-ingestion` lands `source`'s tables in.
pub fn namespace(source: &DataSourceView) -> String {
    format!("cdc_{}", source.id.replace('-', ""))
}
