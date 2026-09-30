//! Application layer: use cases orchestrating the user domain via its ports.

use std::sync::Arc;

use tokio::sync::OnceCell;

use crate::user::domain::{
    NewUser, PasswordHasher, Role, TokenService, User, UserError, UserId, UserRepository,
    UserUpdate, validate_password,
};

/// Orchestrates account management and authentication.
///
/// Depends only on the [`UserRepository`], [`PasswordHasher`] and [`TokenService`] ports, so it
/// stays agnostic to how users are stored, how passwords are hashed and what a token looks like.
pub struct UserService {
    repository: Arc<dyn UserRepository>,
    hasher: Arc<dyn PasswordHasher>,
    tokens: Arc<dyn TokenService>,
    /// A hash to verify against when the username is unknown, so a failed login takes as long
    /// whether or not the account exists.
    decoy_hash: OnceCell<String>,
}

impl UserService {
    pub fn new(
        repository: Arc<dyn UserRepository>,
        hasher: Arc<dyn PasswordHasher>,
        tokens: Arc<dyn TokenService>,
    ) -> Self {
        Self {
            repository,
            hasher,
            tokens,
            decoy_hash: OnceCell::new(),
        }
    }

    pub async fn create(&self, new: NewUser) -> Result<User, UserError> {
        new.validate()?;
        self.ensure_username_available(&new.username, None).await?;
        let hash = self.hasher.hash(&new.password).await?;
        let user = User::register(new, hash);
        self.repository.save(&user).await?;
        Ok(user)
    }

    /// Creates an admin from `username`/`password` if no user exists yet. Returns whether it did.
    pub async fn bootstrap_admin(&self, username: &str, password: &str) -> Result<bool, UserError> {
        if !self.repository.list().await?.is_empty() {
            return Ok(false);
        }
        self.create(NewUser {
            username: username.to_string(),
            password: password.to_string(),
            role: Role::Admin,
            project_ids: Vec::new(),
        })
        .await?;
        Ok(true)
    }

    /// Whether at least one account exists.
    pub async fn has_users(&self) -> Result<bool, UserError> {
        Ok(!self.repository.list().await?.is_empty())
    }

    pub async fn list(&self) -> Result<Vec<User>, UserError> {
        self.repository.list().await
    }

    pub async fn get(&self, id: UserId) -> Result<User, UserError> {
        self.repository
            .find_by_id(id)
            .await?
            .ok_or(UserError::NotFound(id))
    }

    pub async fn update(&self, id: UserId, update: UserUpdate) -> Result<User, UserError> {
        let mut user = self.get(id).await?;

        if let Some(role) = update.role {
            if user.role == Role::Admin && role != Role::Admin {
                self.ensure_another_admin(id).await?;
            }
            user.role = role;
        }
        if let Some(password) = update.password {
            validate_password(&password)?;
            user.password_hash = self.hasher.hash(&password).await?;
        }
        if let Some(project_ids) = update.project_ids {
            user.project_ids = project_ids;
        }

        self.repository.save(&user).await?;
        Ok(user)
    }

    pub async fn remove(&self, id: UserId) -> Result<(), UserError> {
        let user = self.get(id).await?;
        if user.role == Role::Admin {
            self.ensure_another_admin(id).await?;
        }
        self.repository.delete(id).await
    }

    /// Checks `username`/`password` and, on success, returns the account with a fresh token.
    /// Unknown usernames and wrong passwords are indistinguishable to the caller.
    pub async fn authenticate(
        &self,
        username: &str,
        password: &str,
    ) -> Result<(User, String), UserError> {
        let username = username.trim();
        let found = self
            .repository
            .list()
            .await?
            .into_iter()
            .find(|user| user.username.eq_ignore_ascii_case(username));

        let Some(user) = found else {
            let decoy = self
                .decoy_hash
                .get_or_try_init(|| self.hasher.hash("pipa-decoy-password"))
                .await?;
            self.hasher.verify(password, decoy).await;
            return Err(UserError::InvalidCredentials);
        };

        if !self.hasher.verify(password, &user.password_hash).await {
            return Err(UserError::InvalidCredentials);
        }
        let token = self.tokens.issue(&user)?;
        Ok((user, token))
    }

    /// Resolves a bearer token to its account. The account is re-read on every call, so a role
    /// change, project reassignment or deletion takes effect immediately rather than when the
    /// token expires.
    pub async fn verify_token(&self, token: &str) -> Result<User, UserError> {
        let id = self.tokens.verify(token)?;
        self.repository
            .find_by_id(id)
            .await?
            .ok_or(UserError::InvalidToken)
    }

    /// Rejects a username already held by another user (case-insensitive, trimmed). `exclude`
    /// is the user being changed, which may keep its own name.
    async fn ensure_username_available(
        &self,
        username: &str,
        exclude: Option<UserId>,
    ) -> Result<(), UserError> {
        let username = username.trim();
        let taken =
            self.repository.list().await?.into_iter().any(|user| {
                Some(user.id) != exclude && user.username.eq_ignore_ascii_case(username)
            });
        if taken {
            return Err(UserError::DuplicateUsername(username.to_string()));
        }
        Ok(())
    }

    /// Fails with `LastAdmin` unless some admin other than `id` exists.
    async fn ensure_another_admin(&self, id: UserId) -> Result<(), UserError> {
        let other_admin = self
            .repository
            .list()
            .await?
            .into_iter()
            .any(|user| user.id != id && user.role == Role::Admin);
        if !other_admin {
            return Err(UserError::LastAdmin);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::*;

    #[derive(Default)]
    struct InMemoryUsers(Mutex<HashMap<UserId, User>>);

    #[async_trait]
    impl UserRepository for InMemoryUsers {
        async fn save(&self, user: &User) -> Result<(), UserError> {
            self.0.lock().unwrap().insert(user.id, user.clone());
            Ok(())
        }
        async fn find_by_id(&self, id: UserId) -> Result<Option<User>, UserError> {
            Ok(self.0.lock().unwrap().get(&id).cloned())
        }
        async fn list(&self) -> Result<Vec<User>, UserError> {
            Ok(self.0.lock().unwrap().values().cloned().collect())
        }
        async fn delete(&self, id: UserId) -> Result<(), UserError> {
            self.0.lock().unwrap().remove(&id);
            Ok(())
        }
    }

    /// Reversible stand-in for a real hash, so the tests don't pay for Argon2.
    struct FakeHasher;

    #[async_trait]
    impl PasswordHasher for FakeHasher {
        async fn hash(&self, password: &str) -> Result<String, UserError> {
            Ok(format!("hashed:{password}"))
        }
        async fn verify(&self, password: &str, hash: &str) -> bool {
            hash == format!("hashed:{password}")
        }
    }

    /// The token is just the user id.
    struct FakeTokens;

    impl TokenService for FakeTokens {
        fn issue(&self, user: &User) -> Result<String, UserError> {
            Ok(user.id.to_string())
        }
        fn verify(&self, token: &str) -> Result<UserId, UserError> {
            token
                .parse()
                .map(UserId)
                .map_err(|_| UserError::InvalidToken)
        }
    }

    fn service() -> UserService {
        UserService::new(
            Arc::new(InMemoryUsers::default()),
            Arc::new(FakeHasher),
            Arc::new(FakeTokens),
        )
    }

    fn new_user(username: &str, role: Role) -> NewUser {
        NewUser {
            username: username.to_string(),
            password: "correct horse".to_string(),
            role,
            project_ids: Vec::new(),
        }
    }

    #[tokio::test]
    async fn create_rejects_duplicate_usernames_case_insensitively() {
        let service = service();
        service.create(new_user("Alice", Role::User)).await.unwrap();
        let err = service
            .create(new_user(" alice ", Role::User))
            .await
            .unwrap_err();
        assert!(matches!(err, UserError::DuplicateUsername(_)));
    }

    #[tokio::test]
    async fn create_rejects_short_passwords() {
        let mut new = new_user("alice", Role::User);
        new.password = "short".to_string();
        let err = service().create(new).await.unwrap_err();
        assert!(matches!(err, UserError::InvalidField(_)));
    }

    #[tokio::test]
    async fn authenticate_accepts_the_right_password_only() {
        let service = service();
        service.create(new_user("alice", Role::User)).await.unwrap();

        let (user, token) = service
            .authenticate("ALICE", "correct horse")
            .await
            .unwrap();
        assert_eq!(user.username, "alice");
        assert_eq!(service.verify_token(&token).await.unwrap().id, user.id);

        assert!(matches!(
            service.authenticate("alice", "wrong").await,
            Err(UserError::InvalidCredentials)
        ));
        assert!(matches!(
            service.authenticate("nobody", "correct horse").await,
            Err(UserError::InvalidCredentials)
        ));
    }

    #[tokio::test]
    async fn token_stops_working_once_the_user_is_removed() {
        let service = service();
        service.create(new_user("root", Role::Admin)).await.unwrap();
        let alice = service.create(new_user("alice", Role::User)).await.unwrap();
        let (_, token) = service
            .authenticate("alice", "correct horse")
            .await
            .unwrap();

        service.remove(alice.id).await.unwrap();
        assert!(matches!(
            service.verify_token(&token).await,
            Err(UserError::InvalidToken)
        ));
    }

    #[tokio::test]
    async fn token_reflects_role_changes_immediately() {
        let service = service();
        service.create(new_user("root", Role::Admin)).await.unwrap();
        let alice = service
            .create(new_user("alice", Role::Admin))
            .await
            .unwrap();
        let (_, token) = service
            .authenticate("alice", "correct horse")
            .await
            .unwrap();

        service
            .update(
                alice.id,
                UserUpdate {
                    role: Some(Role::User),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(service.verify_token(&token).await.unwrap().role, Role::User);
    }

    #[tokio::test]
    async fn the_last_admin_cannot_be_removed_or_demoted() {
        let service = service();
        let root = service.create(new_user("root", Role::Admin)).await.unwrap();

        assert!(matches!(
            service.remove(root.id).await,
            Err(UserError::LastAdmin)
        ));
        let demote = UserUpdate {
            role: Some(Role::User),
            ..Default::default()
        };
        assert!(matches!(
            service.update(root.id, demote).await,
            Err(UserError::LastAdmin)
        ));

        // With a second admin in place, the first can go.
        service
            .create(new_user("second", Role::Admin))
            .await
            .unwrap();
        service.remove(root.id).await.unwrap();
    }

    #[tokio::test]
    async fn bootstrap_admin_only_runs_on_an_empty_store() {
        let service = service();
        assert!(
            service
                .bootstrap_admin("root", "correct horse")
                .await
                .unwrap()
        );
        assert!(
            !service
                .bootstrap_admin("other", "correct horse")
                .await
                .unwrap()
        );
        let users = service.list().await.unwrap();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].role, Role::Admin);
    }
}
