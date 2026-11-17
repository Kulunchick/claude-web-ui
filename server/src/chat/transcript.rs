use super::parse_native_jsonl_records;
use crate::AppState;
use crate::auth::{AppError, ProjectContext};
use axum::{
    Extension,
    extract::{
        Path, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::Response,
};
use claude_web_ui_shared::{accounts::CliProvider, chat::NativeTranscriptEntryDto};
use serde_json::json;
use std::{io, path::PathBuf, time::Duration};
use tokio::{
    fs::{self, File},
    io::{AsyncReadExt, AsyncSeekExt, SeekFrom},
    time::{MissedTickBehavior, interval},
};

const MAX_RECORD: usize = 1024 * 1024;

pub(super) async fn websocket(
    State(state): State<AppState>,
    Extension(ctx): Extension<ProjectContext>,
    Path((_project_id, session_id)): Path<(String, String)>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, AppError> {
    let (provider, path): (String, Option<String>) = sqlx::query_as(
        "SELECT provider, transcript_path FROM chat_sessions WHERE project_id = ? AND id = ?",
    )
    .bind(&ctx.project.id)
    .bind(&session_id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::not_found("chat session not found"))?;
    let provider: CliProvider = provider.parse().map_err(AppError::internal)?;
    let path = path.ok_or_else(|| AppError::not_found("session has no native transcript"))?;
    let root = state.config.data_dir.join("shared").join(provider.as_str());
    let file = open_transcript(root, PathBuf::from(path))
        .await
        .map_err(|_| AppError::not_found("native transcript unavailable"))?;
    Ok(upgrade.on_upgrade(move |socket| send_updates(socket, file, session_id)))
}

async fn open_transcript(root: PathBuf, path: PathBuf) -> io::Result<File> {
    let root = fs::canonicalize(root).await?;
    let path = fs::canonicalize(path).await?;
    let inside_provider =
        path.starts_with(root.join("projects")) || path.starts_with(root.join("sessions"));
    if !inside_provider
        || path.extension().and_then(|extension| extension.to_str()) != Some("jsonl")
        || !fs::metadata(&path).await?.is_file()
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "invalid transcript file",
        ));
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let file = options.open(path).await?;
    if !file.metadata().await?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "invalid transcript file",
        ));
    }
    Ok(file)
}

struct Tail {
    file: File,
    offset: u64,
    pending: Vec<u8>,
    oversized: bool,
}

impl Tail {
    async fn new(mut file: File) -> io::Result<Self> {
        let offset = file.seek(SeekFrom::End(0)).await?;
        Ok(Self {
            file,
            offset,
            pending: Vec::new(),
            oversized: false,
        })
    }

    async fn read_batch(&mut self) -> io::Result<(bool, Vec<NativeTranscriptEntryDto>)> {
        let reset = self.file.metadata().await?.len() < self.offset;
        if reset {
            self.file.seek(SeekFrom::Start(0)).await?;
            self.offset = 0;
            self.pending.clear();
            self.oversized = false;
        }

        let mut buffer = [0u8; 16 * 1024];
        let len = self.file.read(&mut buffer).await?;
        self.offset += len as u64;
        let mut entries = Vec::new();
        for fragment in buffer[..len].split_inclusive(|byte| *byte == b'\n') {
            if self.pending.len() + fragment.len() > MAX_RECORD {
                self.oversized = true;
                self.pending.clear();
            }
            if !self.oversized {
                self.pending.extend_from_slice(fragment);
            }
            if fragment.ends_with(b"\n") {
                if let Ok(line) = std::str::from_utf8(&self.pending) {
                    entries.extend(parse_native_jsonl_records(line));
                }
                self.pending.clear();
                self.oversized = false;
            }
        }
        Ok((reset, entries))
    }
}

async fn send_updates(mut socket: WebSocket, file: File, session_id: String) {
    let Ok(mut tail) = Tail::new(file).await else {
        return;
    };
    let ready = json!({ "type": "native_transcript_ready", "session_id": session_id });
    if socket
        .send(Message::Text(ready.to_string().into()))
        .await
        .is_err()
    {
        return;
    }

    let mut tick = interval(Duration::from_millis(200));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = tick.tick() => {
                let Ok((reset, entries)) = tail.read_batch().await else {
                    break;
                };
                if reset || !entries.is_empty() {
                    let event = json!({
                        "type": "native_transcript_appended",
                        "session_id": session_id,
                        "reset": reset,
                        "entries": entries,
                    });
                    if socket.send(Message::Text(event.to_string().into())).await.is_err() {
                        break;
                    }
                }
            }
            incoming = socket.recv() => {
                if matches!(incoming, None | Some(Err(_) | Ok(Message::Close(_)))) {
                    break;
                }
            }
        }
    }
}
