use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

const CONTROL_CHANNEL_CAPACITY: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChatControlKind {
    Steer,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcceptedChatControl {
    pub message_id: String,
    pub created_at_ms: i64,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum ChatControlError {
    #[error("Chat control channel is closed")]
    Closed,
    #[error("Chat control was rejected: {0}")]
    Rejected(String),
}

pub struct ChatControlRequest {
    pub kind: ChatControlKind,
    pub prompt: String,
    response: oneshot::Sender<Result<AcceptedChatControl, ChatControlError>>,
}

#[derive(Clone, Debug)]
pub struct ChatControlHandle {
    sender: mpsc::Sender<ChatControlRequest>,
}

pub struct ChatControlReceiver {
    receiver: mpsc::Receiver<ChatControlRequest>,
}

pub fn chat_control_channel() -> (ChatControlHandle, ChatControlReceiver) {
    let (sender, receiver) = mpsc::channel(CONTROL_CHANNEL_CAPACITY);
    (
        ChatControlHandle { sender },
        ChatControlReceiver { receiver },
    )
}

impl ChatControlHandle {
    pub async fn submit(
        &self,
        kind: ChatControlKind,
        prompt: String,
    ) -> Result<AcceptedChatControl, ChatControlError> {
        let (response, accepted) = oneshot::channel();
        self.sender
            .send(ChatControlRequest {
                kind,
                prompt,
                response,
            })
            .await
            .map_err(|_| ChatControlError::Closed)?;
        accepted.await.map_err(|_| ChatControlError::Closed)?
    }
}

impl ChatControlReceiver {
    pub async fn recv(&mut self) -> Option<ChatControlRequest> {
        self.receiver.recv().await
    }

    pub fn try_recv(&mut self) -> Option<ChatControlRequest> {
        self.receiver.try_recv().ok()
    }
}

impl ChatControlRequest {
    pub fn complete(self, result: Result<AcceptedChatControl, ChatControlError>) {
        let _ = self.response.send(result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn control_round_trip_returns_the_durable_message_identity() {
        let (control, mut requests) = chat_control_channel();
        let task = tokio::spawn(async move {
            control
                .submit(ChatControlKind::Steer, "Check the downside".into())
                .await
        });
        let request = requests.recv().await.unwrap();
        assert_eq!(request.kind, ChatControlKind::Steer);
        assert_eq!(request.prompt, "Check the downside");
        request.complete(Ok(AcceptedChatControl {
            message_id: "message-control".into(),
            created_at_ms: 42,
        }));
        assert_eq!(task.await.unwrap().unwrap().message_id, "message-control");
    }
}
