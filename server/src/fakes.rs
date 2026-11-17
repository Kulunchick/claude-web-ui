use crate::traits::{
    CliInvocation, CliRunner, InlineButton, MessengerPlatform, MessengerTransport, OutboundMessage,
    PtyInvocation, PtySession, SpawnedPty, TurnEvent,
};
use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Clone, Default)]
pub struct FakeCliRunner {
    pub invocations: Arc<Mutex<Vec<CliInvocation>>>,
    pub scripted_turns: Arc<Mutex<VecDeque<Vec<TurnEvent>>>>,
    pub approval_decisions: Arc<Mutex<Vec<bool>>>,
    pub pty_invocations: Arc<Mutex<Vec<PtyInvocation>>>,
    pub scripted_pty_outputs: Arc<Mutex<VecDeque<Vec<u8>>>>,
    pub pty_inputs: Arc<Mutex<Vec<Vec<u8>>>>,
    pub pty_resizes: Arc<Mutex<Vec<(u16, u16)>>>,
}

impl FakeCliRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_turn(&self, events: Vec<TurnEvent>) {
        self.scripted_turns
            .lock()
            .expect("poisoned mutex")
            .push_back(events);
    }

    pub fn recorded_invocations(&self) -> Vec<CliInvocation> {
        self.invocations.lock().expect("poisoned mutex").clone()
    }

    pub fn push_pty_output(&self, bytes: Vec<u8>) {
        self.scripted_pty_outputs
            .lock()
            .expect("poisoned mutex")
            .push_back(bytes);
    }

    pub fn recorded_pty_invocations(&self) -> Vec<PtyInvocation> {
        self.pty_invocations.lock().expect("poisoned mutex").clone()
    }

    pub fn recorded_pty_inputs(&self) -> Vec<Vec<u8>> {
        self.pty_inputs.lock().expect("poisoned mutex").clone()
    }

    pub fn recorded_pty_resizes(&self) -> Vec<(u16, u16)> {
        self.pty_resizes.lock().expect("poisoned mutex").clone()
    }
}

struct FakePtySession {
    pty_inputs: Arc<Mutex<Vec<Vec<u8>>>>,
    pty_resizes: Arc<Mutex<Vec<(u16, u16)>>>,
    output_tx: mpsc::UnboundedSender<Vec<u8>>,
    exit_tx: Mutex<Option<oneshot::Sender<i32>>>,
}

#[async_trait]
impl PtySession for FakePtySession {
    async fn write_input(&self, data: &[u8]) -> Result<(), String> {
        self.pty_inputs
            .lock()
            .expect("poisoned mutex")
            .push(data.to_vec());
        if data.contains(&b'\r') || data.contains(&b'\n') {
            let _ = self.output_tx.send(b"Authentication complete.\r\n".to_vec());
            if let Some(tx) = self.exit_tx.lock().expect("poisoned mutex").take() {
                let _ = tx.send(0);
            }
        }
        Ok(())
    }

    async fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        self.pty_resizes
            .lock()
            .expect("poisoned mutex")
            .push((cols, rows));
        Ok(())
    }

    async fn close(&self) -> Result<(), String> {
        if let Some(tx) = self.exit_tx.lock().expect("poisoned mutex").take() {
            let _ = tx.send(0);
        }
        Ok(())
    }
}

#[async_trait]
impl CliRunner for FakeCliRunner {
    async fn run_turn(
        &self,
        invocation: CliInvocation,
        event_tx: mpsc::UnboundedSender<TurnEvent>,
        mut approval_rx: mpsc::UnboundedReceiver<bool>,
    ) -> Result<(), String> {
        self.invocations
            .lock()
            .expect("poisoned mutex")
            .push(invocation);

        let events = self
            .scripted_turns
            .lock()
            .expect("poisoned mutex")
            .pop_front()
            .unwrap_or_else(|| {
                vec![TurnEvent::TurnComplete {
                    session_id: None,
                    result_text: String::new(),
                }]
            });

        for event in events {
            let needs_approval = matches!(event, TurnEvent::PermissionRequest { .. });
            let _ = event_tx.send(event);
            if needs_approval {
                let approved = approval_rx.recv().await.unwrap_or(false);
                self.approval_decisions
                    .lock()
                    .expect("poisoned mutex")
                    .push(approved);
                if !approved {
                    let _ = event_tx.send(TurnEvent::TurnError {
                        message: "permission denied".to_string(),
                    });
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    async fn spawn_pty(&self, invocation: PtyInvocation) -> Result<SpawnedPty, String> {
        let initial_bytes = self
            .scripted_pty_outputs
            .lock()
            .expect("poisoned mutex")
            .pop_front()
            .unwrap_or_else(|| {
                format!(
                    "Starting {} OAuth login in {}...\r\n",
                    invocation.provider.display_name(),
                    invocation.working_dir.display()
                )
                .into_bytes()
            });

        self.pty_invocations
            .lock()
            .expect("poisoned mutex")
            .push(invocation);

        let (output_tx, output_rx) = mpsc::unbounded_channel();
        let (exit_tx, exit_rx) = oneshot::channel();
        let _ = output_tx.send(initial_bytes);

        Ok(SpawnedPty {
            session: Box::new(FakePtySession {
                pty_inputs: self.pty_inputs.clone(),
                pty_resizes: self.pty_resizes.clone(),
                output_tx,
                exit_tx: Mutex::new(Some(exit_tx)),
            }),
            output_rx,
            exit_rx,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedSentMessage {
    pub message_id: String,
    pub message: OutboundMessage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedEditedMessage {
    pub platform: MessengerPlatform,
    pub chat_id: String,
    pub message_id: String,
    pub text: String,
    pub buttons: Vec<Vec<InlineButton>>,
}

#[derive(Debug, Clone, Default)]
pub struct FakeMessengerTransport {
    pub sent: Arc<Mutex<Vec<RecordedSentMessage>>>,
    pub edited: Arc<Mutex<Vec<RecordedEditedMessage>>>,
    next_id: Arc<Mutex<u64>>,
}

impl FakeMessengerTransport {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sent_messages(&self) -> Vec<RecordedSentMessage> {
        self.sent.lock().expect("poisoned mutex").clone()
    }

    pub fn edited_messages(&self) -> Vec<RecordedEditedMessage> {
        self.edited.lock().expect("poisoned mutex").clone()
    }
}

#[async_trait]
impl MessengerTransport for FakeMessengerTransport {
    async fn send_message(&self, message: OutboundMessage) -> Result<String, String> {
        let mut guard = self.next_id.lock().expect("poisoned mutex");
        *guard += 1;
        let id = format!("msg-{}", *guard);
        self.sent
            .lock()
            .expect("poisoned mutex")
            .push(RecordedSentMessage {
                message_id: id.clone(),
                message,
            });
        Ok(id)
    }

    async fn edit_message(
        &self,
        platform: MessengerPlatform,
        chat_id: String,
        message_id: String,
        text: String,
        buttons: Vec<Vec<InlineButton>>,
    ) -> Result<(), String> {
        self.edited
            .lock()
            .expect("poisoned mutex")
            .push(RecordedEditedMessage {
                platform,
                chat_id,
                message_id,
                text,
                buttons,
            });
        Ok(())
    }
}
