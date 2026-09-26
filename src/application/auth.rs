//! 管理员认证用例:初始化、登录、退出、会话校验。
//! 密码 ≥12 Unicode 标量且两次一致;Argon2id;认证失败统一错误(FR-001)。

use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::adapters::sqlite::db::{DbError, DbThread};
use crate::adapters::sqlite::repos::{admin, sessions};
use crate::domain::ids;

pub const MIN_PASSWORD_CHARS: usize = 12;
const TOKEN_LEN: usize = 32;

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("管理员已初始化,不能重复创建")]
    AlreadyInitialized,
    #[error("密码至少需要 {MIN_PASSWORD_CHARS} 个字符")]
    PasswordTooShort,
    #[error("两次输入的密码不一致")]
    PasswordMismatch,
    #[error("用户名或密码错误")]
    InvalidCredentials,
    #[error("尚未初始化管理员")]
    NotInitialized,
    #[error("会话无效或已过期")]
    SessionInvalid,
    #[error("CSRF 校验失败")]
    CsrfRejected,
    #[error("凭据修改失败:当前密码错误或没有可变更项")]
    CredentialChangeFailed,
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("密码哈希失败:{0}")]
    Hash(String),
}

pub struct IssuedSession {
    /// 写入 HttpOnly Cookie 的原始 token;只此一次返回,服务端存哈希。
    pub cookie_token: String,
    /// 返回给前端保存在内存中的 CSRF token。
    pub csrf_token: String,
    pub admin_id: String,
    pub expires_at_ms: i64,
}

pub struct SessionIdentity {
    pub admin_id: String,
}

#[derive(Clone)]
pub struct AdminAuth {
    db: DbThread,
}

impl AdminAuth {
    pub fn new(db: DbThread) -> Self {
        Self { db }
    }

    pub async fn is_initialized(&self) -> Result<bool, AuthError> {
        let row = self.db.call(|conn| admin::find_admin(conn)).await??;
        Ok(row.is_some())
    }

    /// 初始化唯一管理员并直接登录。
    pub async fn initialize(
        &self,
        password: &str,
        confirmation: &str,
    ) -> Result<IssuedSession, AuthError> {
        if password.chars().count() < MIN_PASSWORD_CHARS {
            return Err(AuthError::PasswordTooShort);
        }
        if password != confirmation {
            return Err(AuthError::PasswordMismatch);
        }
        let hash = hash_password(password).map_err(AuthError::Hash)?;
        let admin_id = ids::new_id("adm");
        let admin_id_for_call = admin_id.clone();
        let inserted = self
            .db
            .call(move |conn| admin::insert_single_admin(conn, &admin_id_for_call, "admin", &hash))
            .await??;
        if inserted.is_none() {
            return Err(AuthError::AlreadyInitialized);
        }
        self.issue_session(&admin_id).await
    }

    pub async fn login(&self, password: &str) -> Result<IssuedSession, AuthError> {
        let row = self.db.call(|conn| admin::find_admin(conn)).await??;
        let Some(row) = row else {
            return Err(AuthError::NotInitialized);
        };
        let ok = verify_password(&row.password_hash, password).map_err(AuthError::Hash)?;
        if !ok {
            return Err(AuthError::InvalidCredentials);
        }
        self.issue_session(&row.id).await
    }

    pub async fn logout(&self, cookie_token: &str) -> Result<(), AuthError> {
        let token_hash = hash_hex(cookie_token.as_bytes());
        self.db
            .call(move |conn| sessions::revoke_session(conn, &token_hash))
            .await??;
        Ok(())
    }

    /// 校验 Cookie 会话;提供 csrf 时同时校验 CSRF token。
    pub async fn validate(
        &self,
        cookie_token: &str,
        csrf: Option<&str>,
    ) -> Result<SessionIdentity, AuthError> {
        let token_hash = hash_hex(cookie_token.as_bytes());
        let row = self
            .db
            .call(move |conn| sessions::find_valid_session(conn, &token_hash))
            .await??;
        let row = row.ok_or(AuthError::SessionInvalid)?;
        if let Some(csrf) = csrf
            && hash_hex(csrf.as_bytes()) != row.csrf_hash
        {
            return Err(AuthError::CsrfRejected);
        }
        Ok(SessionIdentity {
            admin_id: row.admin_id,
        })
    }

    /// 撤销全部会话(改密/恢复场景)。
    pub async fn revoke_all(&self) -> Result<(), AuthError> {
        self.db
            .call(|conn| sessions::revoke_all_sessions(conn))
            .await??;
        Ok(())
    }

    /// 007 US7 修改管理员凭据(FR-072):argon2 验证当前密码(错 →
    /// CredentialChangeFailed);新密码沿用既有强度校验(≥12);成功后
    /// **撤销除 keep_session_token 外全部会话**(当前会话保留)。
    /// new_username/new_password 为 None 或空串 = 不修改该项。
    #[allow(clippy::too_many_arguments)]
    pub async fn change_credentials(
        &self,
        current_password: &str,
        new_username: Option<&str>,
        new_password: Option<&str>,
        keep_session_token: &str,
    ) -> Result<(), AuthError> {
        let row = self.db.call(|conn| admin::find_admin(conn)).await??;
        let Some(row) = row else {
            return Err(AuthError::NotInitialized);
        };
        // 当前密码验证(统一凭据失败语义,不区分"未初始化/密码错")
        let ok = verify_password(&row.password_hash, current_password).map_err(AuthError::Hash)?;
        if !ok {
            return Err(AuthError::CredentialChangeFailed);
        }
        // 新密码:空 = 不修改;非空走既有强度校验
        let new_hash = match new_password.map(str::trim).filter(|p| !p.is_empty()) {
            None => None,
            Some(p) => {
                if p.chars().count() < MIN_PASSWORD_CHARS {
                    return Err(AuthError::PasswordTooShort);
                }
                Some(hash_password(p).map_err(AuthError::Hash)?)
            }
        };
        // 新用户名:空/与现值相同 = 不修改;UNIQUE 冲突防御映射凭据失败
        let new_name = new_username
            .map(str::trim)
            .filter(|u| !u.is_empty() && *u != row.username)
            .map(str::to_string);
        if new_hash.is_none() && new_name.is_none() {
            return Err(AuthError::CredentialChangeFailed);
        }
        let keep_hash = hash_hex(keep_session_token.as_bytes());
        let admin_id = row.id;
        self.db
            .call(move |conn| -> rusqlite::Result<()> {
                admin::update_credentials(
                    conn,
                    &admin_id,
                    new_name.as_deref(),
                    new_hash.as_deref(),
                )?;
                sessions::revoke_all_except(conn, &keep_hash)?;
                Ok(())
            })
            .await??;
        Ok(())
    }

    /// 会话查询时轮换 CSRF:服务端只存哈希,每次返回新原文给当前页面。
    pub async fn rotate_csrf(&self, cookie_token: &str) -> Result<String, AuthError> {
        let token_hash = hash_hex(cookie_token.as_bytes());
        let new_csrf = random_hex(TOKEN_LEN);
        let new_hash = hex::encode(Sha256::digest(new_csrf.as_bytes()));
        let updated = self
            .db
            .call(move |conn| sessions::rotate_csrf(conn, &token_hash, &new_hash))
            .await??;
        if updated {
            Ok(new_csrf)
        } else {
            Err(AuthError::SessionInvalid)
        }
    }

    async fn issue_session(&self, admin_id: &str) -> Result<IssuedSession, AuthError> {
        let cookie_token = random_hex(TOKEN_LEN);
        let csrf_token = random_hex(TOKEN_LEN);
        let token_hash = hash_hex(cookie_token.as_bytes());
        let csrf_hash = hash_hex(csrf_token.as_bytes());
        let admin_owned = admin_id.to_string();
        let admin_for_call = admin_owned.clone();
        let created = self
            .db
            .call(move |conn| {
                sessions::insert_session(
                    conn,
                    &token_hash,
                    &admin_for_call,
                    &csrf_hash,
                    sessions::SESSION_TTL_MS,
                )
            })
            .await??;
        Ok(IssuedSession {
            cookie_token,
            csrf_token,
            admin_id: admin_owned,
            expires_at_ms: created + sessions::SESSION_TTL_MS,
        })
    }
}

fn random_hex(n: usize) -> String {
    let mut buf = vec![0u8; n];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

fn hash_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut rand::rngs::OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

fn verify_password(stored: &str, password: &str) -> Result<bool, String> {
    let parsed = PasswordHash::new(stored).map_err(|e| e.to_string())?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

/// CLI 直接哈希(init-admin 停机独占路径;不经过 DbThread)。
pub fn hash_password_for_cli(password: &str) -> Result<String, String> {
    hash_password(password)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup() -> (tempfile::TempDir, AdminAuth) {
        let dir = tempfile::tempdir().unwrap();
        let db = DbThread::spawn(&dir.path().join("auth.db")).unwrap();
        let dir_path = dir.path().to_path_buf();
        db.call(move |conn| crate::adapters::sqlite::migrations::apply(conn, &dir_path))
            .await
            .unwrap()
            .unwrap();
        (dir, AdminAuth::new(db))
    }

    #[tokio::test]
    async fn initialize_login_logout_flow() {
        let (_dir, auth) = setup().await;
        assert!(!auth.is_initialized().await.unwrap());

        let short = auth.initialize("short", "short").await;
        assert!(matches!(short, Err(AuthError::PasswordTooShort)));
        let mismatch = auth
            .initialize("a-long-password-123", "a-long-password-456")
            .await;
        assert!(matches!(mismatch, Err(AuthError::PasswordMismatch)));

        let _s = auth
            .initialize("a-long-password-123", "a-long-password-123")
            .await
            .unwrap();
        assert!(auth.is_initialized().await.unwrap());

        // 重复初始化被拒
        assert!(matches!(
            auth.initialize("another-long-pass-456", "another-long-pass-456")
                .await,
            Err(AuthError::AlreadyInitialized)
        ));

        // 登录 + CSRF 校验
        let bad = auth.login("wrong-password-xxx").await;
        assert!(matches!(bad, Err(AuthError::InvalidCredentials)));
        let ok = auth.login("a-long-password-123").await.unwrap();
        let id = auth
            .validate(&ok.cookie_token, Some(&ok.csrf_token))
            .await
            .unwrap();
        assert!(id.admin_id.starts_with("adm_"));
        assert!(matches!(
            auth.validate(&ok.cookie_token, Some("wrong-csrf")).await,
            Err(AuthError::CsrfRejected)
        ));

        auth.logout(&ok.cookie_token).await.unwrap();
        assert!(matches!(
            auth.validate(&ok.cookie_token, Some(&ok.csrf_token)).await,
            Err(AuthError::SessionInvalid)
        ));
    }

    #[tokio::test]
    async fn change_credentials_keeps_only_current_session() {
        let (_dir, auth) = setup().await;
        auth.initialize("a-long-password-123", "a-long-password-123")
            .await
            .unwrap();
        let current = auth.login("a-long-password-123").await.unwrap();
        let other = auth.login("a-long-password-123").await.unwrap();

        // 当前密码错误 → CredentialChangeFailed,两会话不受影响
        assert!(matches!(
            auth.change_credentials(
                "wrong-current-password",
                Some("admin"),
                Some("b-long-password-456"),
                &current.cookie_token,
            )
            .await,
            Err(AuthError::CredentialChangeFailed)
        ));
        assert!(auth.validate(&other.cookie_token, None).await.is_ok());

        // 新密码过短 → 既有强度校验
        assert!(matches!(
            auth.change_credentials(
                "a-long-password-123",
                None,
                Some("short"),
                &current.cookie_token,
            )
            .await,
            Err(AuthError::PasswordTooShort)
        ));

        // 成功:当前会话保留、其他会话全部失效、新密码可登录
        auth.change_credentials(
            "a-long-password-123",
            Some("boss"),
            Some("b-long-password-456"),
            &current.cookie_token,
        )
        .await
        .unwrap();
        assert!(auth.validate(&current.cookie_token, None).await.is_ok());
        assert!(matches!(
            auth.validate(&other.cookie_token, None).await,
            Err(AuthError::SessionInvalid)
        ));
        let relogin = auth.login("b-long-password-456").await.unwrap();
        let identity = auth.validate(&relogin.cookie_token, Some(&relogin.csrf_token)).await.unwrap();
        assert_eq!(identity.admin_id, current.admin_id);
        // 用户名已更新且旧密码失效
        assert!(matches!(
            auth.login("a-long-password-123").await,
            Err(AuthError::InvalidCredentials)
        ));
    }
}
