use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::Duration;

use ractor::{Actor, ActorProcessingErr, ActorRef, RpcReplyPort};

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WezTermMux {
    program: PathBuf,
}

impl WezTermMux {
    pub fn from_environment() -> Self {
        let program = std::env::var_os("PERSONA_WEZTERM")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("wezterm"));
        Self { program }
    }

    pub fn pane(&self, pane_id: u32) -> TerminalPane {
        TerminalPane {
            backend: self.clone(),
            pane_id,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(["cli", "--prefer-mux"]);
        command
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalPane {
    backend: WezTermMux,
    pane_id: u32,
}

impl TerminalPane {
    pub fn deliver(&self, prompt: &TerminalPrompt) -> Result<DeliveryReceipt> {
        self.send_text(prompt.as_str())?;
        thread::sleep(Duration::from_millis(500));
        self.send_enter()?;
        Ok(DeliveryReceipt {
            pane_id: self.pane_id,
        })
    }

    fn send_text(&self, text: &str) -> Result<()> {
        self.send(SendText::from_text(text))
    }

    fn send_enter(&self) -> Result<()> {
        self.send(SendText::enter())
    }

    fn send(&self, input: SendText<'_>) -> Result<()> {
        let mut command = self.backend.command();
        command.args(["send-text", "--pane-id", &self.pane_id.to_string()]);
        command.args(input.arguments());
        let output = command.output()?;
        if output.status.success() {
            return Ok(());
        }

        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(Error::DeliveryFailed {
            pane_id: self.pane_id,
            detail: if detail.is_empty() {
                format!("exit status {}", output.status)
            } else {
                detail
            },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalPrompt {
    text: String,
}

impl TerminalPrompt {
    pub fn from_text(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    pub fn as_str(&self) -> &str {
        self.text.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryReceipt {
    pane_id: u32,
}

impl DeliveryReceipt {
    pub fn pane_id(&self) -> u32 {
        self.pane_id
    }
}

pub struct TerminalDeliveryActor;

pub enum TerminalDeliveryMessage {
    Deliver {
        pane_id: u32,
        prompt: TerminalPrompt,
        reply: RpcReplyPort<Result<DeliveryReceipt>>,
    },
}

#[ractor::async_trait]
impl Actor for TerminalDeliveryActor {
    type Msg = TerminalDeliveryMessage;
    type State = WezTermMux;
    type Arguments = WezTermMux;

    async fn pre_start(
        &self,
        _myself: ActorRef<Self::Msg>,
        arguments: Self::Arguments,
    ) -> std::result::Result<Self::State, ActorProcessingErr> {
        Ok(arguments)
    }

    async fn handle(
        &self,
        _myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> std::result::Result<(), ActorProcessingErr> {
        match message {
            TerminalDeliveryMessage::Deliver {
                pane_id,
                prompt,
                reply,
            } => {
                let _ = reply.send(state.pane(pane_id).deliver(&prompt));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SendText<'text> {
    mode: SendTextMode<'text>,
}

impl<'text> SendText<'text> {
    fn from_text(text: &'text str) -> Self {
        Self {
            mode: SendTextMode::Text(text),
        }
    }

    fn enter() -> Self {
        Self {
            mode: SendTextMode::Enter,
        }
    }

    fn arguments(&self) -> Vec<&str> {
        match self.mode {
            SendTextMode::Text(text) => vec![text],
            SendTextMode::Enter => vec!["--no-paste", "\r"],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SendTextMode<'text> {
    Text(&'text str),
    Enter,
}
