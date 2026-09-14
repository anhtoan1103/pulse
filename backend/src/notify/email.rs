//! SMTP delivery (docs/pulse-architecture.md #2.6 — email is the only MVP
//! channel).

use crate::config::{EmailConfig, SmtpTls};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, header::ContentType},
    transport::smtp::authentication::Credentials,
};
use std::time::Duration;

const SMTP_TIMEOUT: Duration = Duration::from_secs(20);
const ERROR_MAX_LEN: usize = 200;

#[derive(Debug, PartialEq, Eq)]
pub enum SendError {
    /// Worth retrying: connection problems, timeouts, 4xx SMTP replies.
    Transient(String),
    /// Retrying won't help: 5xx SMTP replies, invalid recipient address.
    Permanent(String),
}

pub struct Mailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl Mailer {
    pub fn new(config: &EmailConfig) -> anyhow::Result<Self> {
        crate::tls::install_crypto_provider();
        let builder = match config.tls {
            SmtpTls::Wrapper => AsyncSmtpTransport::<Tokio1Executor>::relay(&config.host)?,
            SmtpTls::StartTls => {
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.host)?
            }
            SmtpTls::None => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&config.host),
        };
        let mut builder = builder.port(config.port).timeout(Some(SMTP_TIMEOUT));
        if let Some((user, password)) = &config.credentials {
            builder = builder.credentials(Credentials::new(user.clone(), password.clone()));
        }
        let from: Mailbox = config
            .from
            .parse()
            .map_err(|e| anyhow::anyhow!("SMTP_FROM is not a valid address: {e}"))?;

        Ok(Self {
            transport: builder.build(),
            from,
        })
    }

    /// Sends a plain-text email.
    pub async fn send(&self, to: &str, subject: &str, body: String) -> Result<(), SendError> {
        let to: Mailbox = to
            .parse()
            .map_err(|_| SendError::Permanent("recipient address is invalid".into()))?;
        let message = Message::builder()
            .from(self.from.clone())
            .to(to)
            // Defensive: never let CR/LF from data reach a header.
            .subject(subject.replace(['\r', '\n'], " "))
            .header(ContentType::TEXT_PLAIN)
            .body(body)
            .map_err(|e| SendError::Permanent(truncate(&format!("could not build email: {e}"))))?;

        match self.transport.send(message).await {
            Ok(_) => Ok(()),
            Err(e) if e.is_permanent() => Err(SendError::Permanent(truncate(&format!(
                "SMTP server rejected the email: {e}"
            )))),
            Err(e) => Err(SendError::Transient(truncate(&format!(
                "SMTP delivery failed: {e}"
            )))),
        }
    }
}

fn truncate(text: &str) -> String {
    text.chars().take(ERROR_MAX_LEN).collect()
}
