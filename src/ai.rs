use anyhow::{Result, anyhow};
use aws_sdk_bedrockruntime::config::ProvideCredentials;
use aws_sdk_bedrockruntime::types::{ContentBlock, ConversationRole, Message};
use log::debug;

use crate::summary::strip_line_prefix;

const DEFAULT_MODEL_ID: &str = "us.amazon.nova-2-lite-v1:0";

pub struct BedrockSummarizer {
    client: aws_sdk_bedrockruntime::Client,
    model_id: String,
}

impl BedrockSummarizer {
    /// Returns `None` when AWS credentials are not configured. The CLI runs without
    /// credentials in local use; the Lambda is always credentialed.
    pub async fn from_env() -> Option<Self> {
        let config = aws_config::from_env().load().await;
        let provider = config.credentials_provider()?;
        if let Err(e) = provider.provide_credentials().await {
            debug!("AWS credentials unavailable; skipping Bedrock summaries: {e}");
            return None;
        }
        let model_id =
            std::env::var("BEDROCK_MODEL_ID").unwrap_or_else(|_| DEFAULT_MODEL_ID.to_owned());
        Some(Self {
            client: aws_sdk_bedrockruntime::Client::new(&config),
            model_id,
        })
    }

    pub async fn generate_summary(&self, header: &str) -> Result<String> {
        let prompt = format!(
            "Create a concise summary title for the following public transit alert, \
             suitable for a calendar event title. The title should be brief (under 60 \
             characters), informative, and focus on the key disruption type and location \
             if relevant. Use Title Case. Do not include the line name (e.g. \"Red Line\", \
             \"Green Line\") or dates and times — the line and date are already shown \
             separately in the calendar. Respond with only the title text, no quotes or \
             explanation.\n\nAlert: {header}"
        );

        let message = Message::builder()
            .role(ConversationRole::User)
            .content(ContentBlock::Text(prompt))
            .build()?;

        let response = self
            .client
            .converse()
            .model_id(&self.model_id)
            .messages(message)
            .send()
            .await?;

        let text = response
            .output()
            .and_then(|o| o.as_message().ok())
            .and_then(|m| m.content().first())
            .and_then(|b| b.as_text().ok())
            .map(|s| strip_line_prefix(s.trim()).to_owned())
            .ok_or_else(|| anyhow!("Unexpected Bedrock response structure"))?;

        debug!("Bedrock summary for {:?}: {:?}", header, text);
        Ok(text)
    }
}
