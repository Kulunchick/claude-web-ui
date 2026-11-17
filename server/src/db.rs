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

        CREATE TABLE IF NOT EXISTS chat_sessions (
            id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            provider TEXT NOT NULL,
            native_session_id TEXT,
            title TEXT NOT NULL,
            approval_mode TEXT NOT NULL DEFAULT 'full_auto',
            source TEXT NOT NULL DEFAULT 'gateway',
            transcript_path TEXT,
            created_by TEXT REFERENCES users(id) ON DELETE SET NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_chat_sessions_project ON chat_sessions(project_id);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_chat_sessions_native
            ON chat_sessions(project_id, provider, native_session_id)
            WHERE native_session_id IS NOT NULL;

        CREATE TABLE IF NOT EXISTS chat_turns (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL REFERENCES chat_sessions(id) ON DELETE CASCADE,
            project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            user_id TEXT NOT NULL,
            provider TEXT NOT NULL,
            account_id TEXT,
            prompt TEXT NOT NULL,
            status TEXT NOT NULL,
            result_text TEXT,
            error_message TEXT,
            pending_permission_json TEXT,
            events_json TEXT NOT NULL DEFAULT '[]',
            created_at TEXT NOT NULL,
            started_at TEXT,
            completed_at TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_chat_turns_session ON chat_turns(session_id, created_at);

        CREATE TABLE IF NOT EXISTS kanban_cards (
            id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            parent_id TEXT REFERENCES kanban_cards(id) ON DELETE SET NULL,
            title TEXT NOT NULL,
            description TEXT NOT NULL,
            lane TEXT NOT NULL CHECK(lane IN ('triage','ready','running','blocked','review','done','archived')),
            position INTEGER NOT NULL,
            assigned_cli TEXT,
            create_git_branch INTEGER NOT NULL DEFAULT 0,
            git_branch_name TEXT,
            git_base TEXT,
            git_diff TEXT,
            session_id TEXT REFERENCES chat_sessions(id) ON DELETE SET NULL,
            turn_id TEXT REFERENCES chat_turns(id) ON DELETE SET NULL,
            blocked_reason TEXT,
            result_summary TEXT,
            created_by TEXT NOT NULL REFERENCES users(id),
            run_user_id TEXT REFERENCES users(id),
            decompose INTEGER NOT NULL DEFAULT 0,
            run_token TEXT,
            active_run INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_kanban_project_lane
            ON kanban_cards(project_id, lane, position);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_kanban_active_run
            ON kanban_cards(active_run) WHERE active_run = 1;

        CREATE TABLE IF NOT EXISTS messenger_pairing_codes (
            code TEXT PRIMARY KEY,
            user_id TEXT NOT NULL UNIQUE REFERENCES users(id) ON DELETE CASCADE,
            expires_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS messenger_pairing_attempts (
            platform TEXT NOT NULL,
            external_user_id TEXT NOT NULL,
            attempted_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_messenger_attempts_time
            ON messenger_pairing_attempts(attempted_at);

        CREATE TABLE IF NOT EXISTS user_messenger_links (
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            platform TEXT NOT NULL,
            external_user_id TEXT NOT NULL,
            dm_chat_id TEXT NOT NULL,
            PRIMARY KEY (platform, external_user_id),
            UNIQUE (user_id, platform)
        );

        CREATE TABLE IF NOT EXISTS messenger_dm_contexts (
            platform TEXT NOT NULL,
            external_user_id TEXT NOT NULL,
            project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            session_id TEXT REFERENCES chat_sessions(id) ON DELETE SET NULL,
            PRIMARY KEY (platform, external_user_id),
            FOREIGN KEY (platform, external_user_id)
                REFERENCES user_messenger_links(platform, external_user_id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS messenger_channels (
            platform TEXT NOT NULL,
            chat_id TEXT NOT NULL,
            project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            notify INTEGER NOT NULL DEFAULT 0,
            notification_thread_id TEXT,
            PRIMARY KEY (platform, chat_id)
        );

        CREATE TABLE IF NOT EXISTS messenger_threads (
            platform TEXT NOT NULL,
            chat_id TEXT NOT NULL,
            thread_id TEXT NOT NULL,
            session_id TEXT NOT NULL UNIQUE REFERENCES chat_sessions(id) ON DELETE CASCADE,
            PRIMARY KEY (platform, chat_id, thread_id),
            FOREIGN KEY (platform, chat_id)
                REFERENCES messenger_channels(platform, chat_id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS messenger_dm_notifications (
            platform TEXT NOT NULL,
            external_user_id TEXT NOT NULL,
            project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            PRIMARY KEY (platform, external_user_id, project_id),
            FOREIGN KEY (platform, external_user_id)
                REFERENCES user_messenger_links(platform, external_user_id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS kanban_dependencies (
            card_id TEXT NOT NULL REFERENCES kanban_cards(id) ON DELETE CASCADE,
            blocker_id TEXT NOT NULL REFERENCES kanban_cards(id) ON DELETE CASCADE,
            PRIMARY KEY (card_id, blocker_id)
        );
        "#,
    )
    .execute(pool)
    .await?;

    Ok(())
}
