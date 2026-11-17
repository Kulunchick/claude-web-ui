use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;
use std::path::Path;
use std::str::FromStr;

pub async fn connect_sqlite_wal(db_path: &Path) -> Result<SqlitePool, sqlx::Error> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).map_err(sqlx::Error::Io)?;
    }

    let url = format!("sqlite://{}", db_path.display());
    let options = SqliteConnectOptions::from_str(&url)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;

    init_schema(&pool).await?;
    Ok(pool)
}

pub async fn init_schema(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            username TEXT NOT NULL UNIQUE,
            email TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            role TEXT NOT NULL,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS user_sessions (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            refresh_token_id TEXT NOT NULL,
            revoked_at TEXT,
            expires_at TEXT NOT NULL,
            created_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_user_sessions_user_id ON user_sessions(user_id);

        CREATE TABLE IF NOT EXISTS projects (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            slug TEXT NOT NULL UNIQUE,
            workspace_path TEXT NOT NULL UNIQUE,
            created_by TEXT NOT NULL REFERENCES users(id),
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS project_memberships (
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            role_override TEXT,
            created_at TEXT NOT NULL,
            PRIMARY KEY (user_id, project_id)
        );

        CREATE TABLE IF NOT EXISTS user_invites (
            id TEXT PRIMARY KEY,
            code TEXT NOT NULL UNIQUE,
            role TEXT NOT NULL,
            created_by TEXT NOT NULL REFERENCES users(id),
            used_by TEXT REFERENCES users(id),
            used_at TEXT,
            expires_at TEXT,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS invite_projects (
            invite_id TEXT NOT NULL REFERENCES user_invites(id) ON DELETE CASCADE,
            project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            PRIMARY KEY (invite_id, project_id)
        );

        CREATE TABLE IF NOT EXISTS audit_logs (
            id TEXT PRIMARY KEY,
            user_id TEXT REFERENCES users(id) ON DELETE SET NULL,
            action TEXT NOT NULL,
            target_type TEXT,
            target_id TEXT,
            details TEXT,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS cli_accounts (
            id TEXT PRIMARY KEY,
            provider TEXT NOT NULL,
            name TEXT NOT NULL,
            slug TEXT NOT NULL,
            profile_dir TEXT NOT NULL,
            created_by TEXT REFERENCES users(id) ON DELETE SET NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            UNIQUE(provider, slug)
        );

        CREATE TABLE IF NOT EXISTS project_active_cli (
            project_id TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
            active_provider TEXT NOT NULL,
            active_account_id TEXT REFERENCES cli_accounts(id) ON DELETE SET NULL,
            updated_by TEXT REFERENCES users(id) ON DELETE SET NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS project_provider_accounts (
            project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            provider TEXT NOT NULL,
            account_id TEXT NOT NULL REFERENCES cli_accounts(id) ON DELETE CASCADE,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (project_id, provider)
        );

        CREATE TABLE IF NOT EXISTS project_member_acl_settings (
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            restrict_accounts INTEGER NOT NULL DEFAULT 0,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (user_id, project_id)
        );

        CREATE TABLE IF NOT EXISTS project_member_allowed_accounts (
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            account_id TEXT NOT NULL REFERENCES cli_accounts(id) ON DELETE CASCADE,
            PRIMARY KEY (user_id, project_id, account_id)
        );
        "#,
    )
    .execute(pool)
    .await?;

    Ok(())
}
