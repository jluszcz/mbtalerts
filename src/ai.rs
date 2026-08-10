//! Bedrock-backed alert summarization: the prompt and the cleanup of the
//! model's reply, over the shared Converse client.

use anyhow::Result;
use jluszcz_rust_utils::bedrock::BedrockClient;
use log::debug;

use crate::summary::strip_line_prefix;

pub struct BedrockSummarizer {
    client: BedrockClient,
}

impl BedrockSummarizer {
    /// Returns `None` when AWS credentials are not configured. The CLI runs without
    /// credentials in local use; the Lambda is always credentialed.
    pub async fn from_env() -> Option<Self> {
        Some(Self {
            client: BedrockClient::from_env_if_credentialed().await?,
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

        let text = strip_line_prefix(self.client.generate(&prompt).await?.trim()).to_owned();

        debug!("Bedrock summary for {header:?}: {text:?}");
        Ok(text)
    }
}
