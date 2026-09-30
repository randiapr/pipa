//! Object-store-backed repository for the `User` aggregate.
//!
//! Persists each user as a JSON object under the `users/` prefix of the shared
//! RustFS/S3-compatible object store, mirroring how projects and data sources are persisted.

use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use object_store::{ObjectStore, ObjectStoreExt, PutPayload, path::Path as ObjectPath};

use crate::user::domain::{User, UserError, UserId, UserRepository};

const PREFIX: &str = "users";

/// Persists `User` aggregates as JSON objects in the configured object store.
pub struct ObjectStoreUserRepository {
    store: Arc<dyn ObjectStore>,
}

impl ObjectStoreUserRepository {
    pub fn new(store: Arc<dyn ObjectStore>) -> Self {
        Self { store }
    }

    fn path_for(id: UserId) -> ObjectPath {
        ObjectPath::from(format!("{PREFIX}/{id}.json"))
    }
}

#[async_trait]
impl UserRepository for ObjectStoreUserRepository {
    async fn save(&self, user: &User) -> Result<(), UserError> {
        let bytes = serde_json::to_vec(user).map_err(|err| UserError::Storage(err.to_string()))?;
        self.store
            .put(&Self::path_for(user.id), PutPayload::from(bytes))
            .await
            .map_err(|err| UserError::Storage(err.to_string()))?;
        Ok(())
    }

    async fn find_by_id(&self, id: UserId) -> Result<Option<User>, UserError> {
        match self.store.get(&Self::path_for(id)).await {
            Ok(result) => {
                let bytes = result
                    .bytes()
                    .await
                    .map_err(|err| UserError::Storage(err.to_string()))?;
                let user = serde_json::from_slice(&bytes)
                    .map_err(|err| UserError::Storage(err.to_string()))?;
                Ok(Some(user))
            }
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            Err(err) => Err(UserError::Storage(err.to_string())),
        }
    }

    async fn list(&self) -> Result<Vec<User>, UserError> {
        let mut listing = self.store.list(Some(&ObjectPath::from(PREFIX)));
        let mut users = Vec::new();

        while let Some(meta) = listing.next().await {
            let meta = meta.map_err(|err| UserError::Storage(err.to_string()))?;
            let bytes = self
                .store
                .get(&meta.location)
                .await
                .map_err(|err| UserError::Storage(err.to_string()))?
                .bytes()
                .await
                .map_err(|err| UserError::Storage(err.to_string()))?;
            let user = serde_json::from_slice(&bytes)
                .map_err(|err| UserError::Storage(err.to_string()))?;
            users.push(user);
        }

        Ok(users)
    }

    async fn delete(&self, id: UserId) -> Result<(), UserError> {
        self.store
            .delete(&Self::path_for(id))
            .await
            .map_err(|err| UserError::Storage(err.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use object_store::memory::InMemory;

    use super::*;
    use crate::user::domain::{NewUser, Role};

    #[tokio::test]
    async fn round_trips_a_user_through_the_store() {
        let repository = ObjectStoreUserRepository::new(Arc::new(InMemory::new()));
        let user = User::register(
            NewUser {
                username: "alice".to_string(),
                password: "unused".to_string(),
                role: Role::User,
                project_ids: Vec::new(),
            },
            "hash".to_string(),
        );

        repository.save(&user).await.unwrap();
        let found = repository.find_by_id(user.id).await.unwrap().unwrap();
        assert_eq!(found.username, "alice");
        assert_eq!(repository.list().await.unwrap().len(), 1);

        repository.delete(user.id).await.unwrap();
        assert!(repository.find_by_id(user.id).await.unwrap().is_none());
    }
}
