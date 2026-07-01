use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    pub password_hash: Option<String>,
    pub google_sub: Option<String>,
    pub role: UserRole,
    pub fcm_token: Option<String>,
    pub timezone: Option<String>,
    pub department: Option<String>,
    pub job_title: Option<String>,
    pub work_start: Option<String>,
    pub work_end: Option<String>,
    pub manager_name: Option<String>,
    pub public_holidays: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    Regular,
    Admin,
}

impl Default for UserRole {
    fn default() -> Self {
        UserRole::Regular
    }
}

/// Payload for creating a new user record.
pub struct NewUser {
    pub email: String,
    pub display_name: String,
    pub password_hash: Option<String>,
    pub google_sub: Option<String>,
    pub role: UserRole,
}

/// Public-safe representation (no password hash).
#[derive(Debug, Serialize)]
pub struct UserProfile {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role: UserRole,
    pub timezone: Option<String>,
    pub department: Option<String>,
    pub job_title: Option<String>,
    pub work_start: Option<String>,
    pub work_end: Option<String>,
    pub manager_name: Option<String>,
    pub public_holidays: Vec<String>,
    pub created_at: DateTime<Utc>,
}

impl From<User> for UserProfile {
    fn from(u: User) -> Self {
        UserProfile {
            id: u.id,
            email: u.email,
            display_name: u.display_name,
            role: u.role,
            timezone: u.timezone,
            department: u.department,
            job_title: u.job_title,
            work_start: u.work_start,
            work_end: u.work_end,
            manager_name: u.manager_name,
            public_holidays: u.public_holidays,
            created_at: u.created_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    fn make_user() -> User {
        User {
            id: Uuid::new_v4(),
            email: "alice@example.com".to_string(),
            display_name: "Alice".to_string(),
            password_hash: Some("hashed_secret".to_string()),
            google_sub: None,
            role: UserRole::Regular,
            fcm_token: None,
            timezone: None,
            department: None,
            job_title: None,
            work_start: None,
            work_end: None,
            manager_name: None,
            public_holidays: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn user_role_default_is_regular() {
        assert_eq!(UserRole::default(), UserRole::Regular);
    }

    #[test]
    fn user_profile_from_user_copies_fields() {
        let user = make_user();
        let id = user.id;
        let email = user.email.clone();
        let display_name = user.display_name.clone();
        let role = user.role.clone();
        let created_at = user.created_at;

        let profile = UserProfile::from(user);

        assert_eq!(profile.id, id);
        assert_eq!(profile.email, email);
        assert_eq!(profile.display_name, display_name);
        assert_eq!(profile.role, role);
        assert_eq!(profile.created_at, created_at);
    }

    #[test]
    fn user_profile_excludes_password_hash() {
        let user = make_user();
        let profile = UserProfile::from(user);
        // UserProfile has only: id, email, display_name, role, created_at.
        // Accessing any of these fields compiles; password_hash is absent.
        let _ = profile.id;
        let _ = &profile.email;
        let _ = &profile.display_name;
        let _ = &profile.role;
        let _ = profile.created_at;
    }
}
