use crate::traits::{
    CliInvocation, CliRunner, InlineButton, MessengerPlatform, MessengerTransport, OutboundMessage,
    TurnEvent,
};
use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Default)]
pub struct FakeCliRunner {
    pub invocations: Arc<Mutex<Vec<CliInvocation>>>,
    pub scripted_turns: Arc<Mutex<VecDeque<Vec<TurnEvent>>>>,
    pub approval_decisions: Arc<Mutex<Vec<bool>>>,
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
