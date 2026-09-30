//! HS256 JSON Web Tokens as the bearer credential handed out at login.

use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::user::domain::{TokenService, User, UserError, UserId};

/// Shortest `JWT_SECRET` accepted, in bytes.
pub const MIN_SECRET_LEN: usize = 32;

/// How long a token stays valid after login.
const TOKEN_TTL_SECS: u64 = 8 * 60 * 60;

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    /// The user id.
    sub: String,
    exp: u64,
}

/// Signs and verifies tokens with a shared secret. Tokens carry only the user id: the role and
/// project assignments are read from the user store on every request (see
/// `UserService::verify_token`), so they can't go stale inside a token.
pub struct JwtTokenService {
    encoding: EncodingKey,
    decoding: DecodingKey,
}

impl JwtTokenService {
    /// Fails if `secret` is shorter than [`MIN_SECRET_LEN`] bytes.
    pub fn new(secret: &str) -> Result<Self, UserError> {
        if secret.len() < MIN_SECRET_LEN {
            return Err(UserError::InvalidField(format!(
                "the token secret must be at least {MIN_SECRET_LEN} bytes"
            )));
        }
        Ok(Self {
            encoding: EncodingKey::from_secret(secret.as_bytes()),
            decoding: DecodingKey::from_secret(secret.as_bytes()),
        })
    }
}

impl TokenService for JwtTokenService {
    fn issue(&self, user: &User) -> Result<String, UserError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or_default();
        let claims = Claims {
            sub: user.id.to_string(),
            exp: now + TOKEN_TTL_SECS,
        };
        encode(&Header::new(Algorithm::HS256), &claims, &self.encoding)
            .map_err(|err| UserError::Storage(format!("failed to sign token: {err}")))
    }

    fn verify(&self, token: &str) -> Result<UserId, UserError> {
        let claims = decode::<Claims>(token, &self.decoding, &Validation::new(Algorithm::HS256))
            .map_err(|_| UserError::InvalidToken)?
            .claims;
        Uuid::parse_str(&claims.sub)
            .map(UserId)
            .map_err(|_| UserError::InvalidToken)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::user::domain::{NewUser, Role};

    const SECRET: &str = "0123456789abcdef0123456789abcdef";

    fn user() -> User {
        User::register(
            NewUser {
                username: "alice".to_string(),
                password: "unused".to_string(),
                role: Role::User,
                project_ids: Vec::new(),
            },
            "hash".to_string(),
        )
    }

    #[test]
    fn round_trips_the_user_id() {
        let service = JwtTokenService::new(SECRET).unwrap();
        let user = user();
        let token = service.issue(&user).unwrap();
        assert_eq!(service.verify(&token).unwrap(), user.id);
    }

    #[test]
    fn rejects_tokens_signed_with_another_secret() {
        let token = JwtTokenService::new(SECRET)
            .unwrap()
            .issue(&user())
            .unwrap();
        let other = JwtTokenService::new("another-secret-another-secret-00").unwrap();
        assert!(matches!(other.verify(&token), Err(UserError::InvalidToken)));
    }

    #[test]
    fn rejects_garbage_and_expired_tokens() {
        let service = JwtTokenService::new(SECRET).unwrap();
        assert!(matches!(
            service.verify("not.a.token"),
            Err(UserError::InvalidToken)
        ));

        let expired = encode(
            &Header::new(Algorithm::HS256),
            &Claims {
                sub: user().id.to_string(),
                exp: 1,
            },
            &EncodingKey::from_secret(SECRET.as_bytes()),
        )
        .unwrap();
        assert!(matches!(
            service.verify(&expired),
            Err(UserError::InvalidToken)
        ));
    }

    #[test]
    fn refuses_a_short_secret() {
        assert!(JwtTokenService::new("too short").is_err());
    }
}
