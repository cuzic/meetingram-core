use std::path::PathBuf;
use std::sync::Arc;

use credential_store::CredentialStore;
use genai::adapter::AdapterKind;
use genai::resolver::Error as ResolverError;
use genai::{Client, ModelIden};

use crate::cli_backend::CliBackend;
use crate::{
    build_claude_oauth_client, build_ollama_client, build_vertex_client,
    credential_store_auth_resolver, CliSummarizer, GenaiSummarizer, Summarizer,
    BEDROCK_API_KEY_ACCOUNT, CLAUDE_API_KEY_ACCOUNT, CLAUDE_VERTEX_CREDENTIALS_ACCOUNT,
    CREDENTIAL_SERVICE, DEEPSEEK_API_KEY_ACCOUNT, GEMINI_API_KEY_ACCOUNT,
    GEMINI_VERTEX_CREDENTIALS_ACCOUNT, GROQ_API_KEY_ACCOUNT, OPENAI_API_KEY_ACCOUNT,
    XAI_API_KEY_ACCOUNT,
};

/// Provider selected in the application’s summary settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryProvider {
    Claude,
    OpenAi,
    Gemini,
    Groq,
    DeepSeek,
    XAi,
    ClaudeVertex,
    GeminiVertex,
    ClaudeBedrock,
    ClaudeCli,
    /// Claude via the Anthropic CLI’s OAuth login.
    ClaudeOAuth,
    Codex,
    /// A local or self-hosted Ollama server.
    Ollama,
}

impl SummaryProvider {
    pub const ALL: [Self; 13] = [
        Self::Claude,
        Self::OpenAi,
        Self::Gemini,
        Self::Groq,
        Self::DeepSeek,
        Self::XAi,
        Self::ClaudeVertex,
        Self::GeminiVertex,
        Self::ClaudeBedrock,
        Self::ClaudeCli,
        Self::ClaudeOAuth,
        Self::Codex,
        Self::Ollama,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::OpenAi => "openai",
            Self::Gemini => "gemini",
            Self::Groq => "groq",
            Self::DeepSeek => "deepseek",
            Self::XAi => "xai",
            Self::ClaudeVertex => "claude-vertex",
            Self::GeminiVertex => "gemini-vertex",
            Self::ClaudeBedrock => "claude-bedrock",
            Self::ClaudeCli => "claude-cli",
            Self::ClaudeOAuth => "claude-oauth",
            Self::Codex => "codex",
            Self::Ollama => "ollama",
        }
    }

    pub fn default_model(self) -> &'static str {
        match self {
            Self::Claude => "claude-sonnet-4-5",
            Self::OpenAi => "gpt-4o-mini",
            Self::Gemini => "gemini-3-flash-preview",
            Self::Groq => "groq::openai/gpt-oss-20b",
            Self::DeepSeek => "deepseek-v4-flash",
            Self::XAi => "grok-4",
            Self::ClaudeVertex => "vertex::claude-sonnet-4-5",
            Self::GeminiVertex => "vertex::gemini-3-flash-preview",
            Self::ClaudeBedrock => "bedrock_api::anthropic.claude-sonnet-4-5-20250929-v1:0",
            Self::ClaudeCli => "sonnet",
            Self::ClaudeOAuth => "claude-sonnet-4-5",
            Self::Codex => "gpt-5.5",
            Self::Ollama => "llama3.2",
        }
    }

    pub fn api_key_account(self) -> Option<&'static str> {
        match self {
            Self::Claude => Some(CLAUDE_API_KEY_ACCOUNT),
            Self::OpenAi => Some(OPENAI_API_KEY_ACCOUNT),
            Self::Gemini => Some(GEMINI_API_KEY_ACCOUNT),
            Self::Groq => Some(GROQ_API_KEY_ACCOUNT),
            Self::DeepSeek => Some(DEEPSEEK_API_KEY_ACCOUNT),
            Self::XAi => Some(XAI_API_KEY_ACCOUNT),
            Self::ClaudeVertex => Some(CLAUDE_VERTEX_CREDENTIALS_ACCOUNT),
            Self::GeminiVertex => Some(GEMINI_VERTEX_CREDENTIALS_ACCOUNT),
            Self::ClaudeBedrock => Some(BEDROCK_API_KEY_ACCOUNT),
            Self::ClaudeCli | Self::ClaudeOAuth | Self::Codex | Self::Ollama => None,
        }
    }

    pub fn from_key(key: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|p| p.key() == key)
            .unwrap_or(Self::Claude)
    }

    pub fn is_vertex(self) -> bool {
        matches!(self, Self::ClaudeVertex | Self::GeminiVertex)
    }

    /// Whether discussion graphs can use this provider.
    pub fn supports_graph(self) -> bool {
        self != Self::Codex
    }

    pub fn cli_backend(self) -> Option<CliBackend> {
        match self {
            Self::ClaudeCli => Some(CliBackend::ClaudeCode),
            Self::Codex => Some(CliBackend::Codex),
            _ => None,
        }
    }
}

/// Chooses the graph override, stored summary model, or provider default.
pub fn resolve_model(
    provider: SummaryProvider,
    stored_summary_model: Option<&str>,
    graph_model_override: Option<&str>,
) -> String {
    graph_model_override
        .filter(|model| !model.trim().is_empty())
        .or_else(|| stored_summary_model.filter(|model| !model.trim().is_empty()))
        .unwrap_or_else(|| provider.default_model())
        .to_owned()
}

/// Configuration failures that callers can map to their own messages.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum BuildSummarizerError {
    #[error("configure Google Vertex AI credentials in settings")]
    MissingVertexCredentials,
    #[error("configure an API key in settings")]
    MissingApiKey,
    #[error("could not read Google Vertex AI credentials")]
    InvalidVertexCredentials,
    #[error("could not build the AI client")]
    Client,
}

/// Builds the configured backend without making a provider request.
/// Reading the credential store can block.
/// For CLI providers, `cli_temp_base` selects an existing base directory for
/// per-request working directories. Pass `None` to use the OS temporary directory.
pub fn build_summarizer<S: CredentialStore + Send + Sync + 'static>(
    provider_key: &str,
    store: Arc<S>,
    ollama_base_url: Option<String>,
    cli_temp_base: Option<PathBuf>,
) -> Result<Box<dyn Summarizer>, BuildSummarizerError> {
    let provider = SummaryProvider::from_key(provider_key);
    if let Some(backend) = provider.cli_backend() {
        return Ok(Box::new(CliSummarizer::new(backend, cli_temp_base)));
    }

    let client = if provider == SummaryProvider::ClaudeOAuth {
        build_claude_oauth_client()
    } else if provider == SummaryProvider::Ollama {
        build_ollama_client(ollama_base_url)
    } else if let Some(account) = provider.api_key_account() {
        if provider.is_vertex() {
            let raw = store
                .load(CREDENTIAL_SERVICE, account)
                .map_err(|_| BuildSummarizerError::MissingVertexCredentials)?;
            let credentials = serde_json::from_str(&raw)
                .map_err(|_| BuildSummarizerError::InvalidVertexCredentials)?;
            build_vertex_client(credentials)
        } else {
            store
                .load(CREDENTIAL_SERVICE, account)
                .map_err(|_| BuildSummarizerError::MissingApiKey)?;
            build_api_key_client(provider, store, account)
        }
    } else {
        Client::builder().build()
    };

    Ok(Box::new(GenaiSummarizer(
        client.map_err(|_| BuildSummarizerError::Client)?,
    )))
}

fn build_api_key_client<S: CredentialStore + Send + Sync + 'static>(
    provider: SummaryProvider,
    store: Arc<S>,
    account: &'static str,
) -> Result<Client, genai::Error> {
    let adapter = AdapterKind::from_model(provider.default_model())?;
    Client::builder()
        // A model override may select another API on the same provider, but
        // must not send the selected provider's key to a different service.
        .with_model_mapper_fn(move |model: ModelIden| {
            if model.adapter_kind == adapter
                || matches!(
                    (provider, model.adapter_kind),
                    (SummaryProvider::OpenAi, AdapterKind::OpenAIResp)
                        | (SummaryProvider::Gemini, AdapterKind::GeminiIx)
                )
            {
                Ok(model)
            } else {
                Err(ResolverError::Custom(
                    "model does not match the configured provider".into(),
                ))
            }
        })
        .with_auth_resolver(credential_store_auth_resolver(store, account))
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_settings_match_desktop() {
        let settings = [
            ("claude", "claude-sonnet-4-5"),
            ("openai", "gpt-4o-mini"),
            ("gemini", "gemini-3-flash-preview"),
            ("groq", "groq::openai/gpt-oss-20b"),
            ("deepseek", "deepseek-v4-flash"),
            ("xai", "grok-4"),
            ("claude-vertex", "vertex::claude-sonnet-4-5"),
            ("gemini-vertex", "vertex::gemini-3-flash-preview"),
            (
                "claude-bedrock",
                "bedrock_api::anthropic.claude-sonnet-4-5-20250929-v1:0",
            ),
            ("claude-cli", "sonnet"),
            ("claude-oauth", "claude-sonnet-4-5"),
            ("codex", "gpt-5.5"),
            ("ollama", "llama3.2"),
        ];
        assert_eq!(SummaryProvider::ALL.len(), settings.len());
        for (key, model) in settings {
            let provider = SummaryProvider::from_key(key);
            assert_eq!(provider.key(), key);
            assert_eq!(provider.default_model(), model);
            assert_eq!(provider.supports_graph(), key != "codex");
        }
        assert_eq!(
            SummaryProvider::from_key("unknown"),
            SummaryProvider::Claude
        );
    }

    #[test]
    fn model_precedence() {
        let provider = SummaryProvider::OpenAi;
        assert_eq!(
            resolve_model(provider, Some("summary"), Some("graph")),
            "graph"
        );
        assert_eq!(
            resolve_model(provider, Some("summary"), Some("")),
            "summary"
        );
        assert_eq!(resolve_model(provider, Some(""), None), "gpt-4o-mini");
        assert_eq!(resolve_model(provider, None, None), "gpt-4o-mini");
    }

    #[tokio::test]
    async fn model_override_cannot_route_api_key_to_another_provider() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(credential_store::EncryptedFileStore::new(dir.path()).unwrap());
        store
            .save(CREDENTIAL_SERVICE, OPENAI_API_KEY_ACCOUNT, "synthetic-key")
            .unwrap();
        let client =
            build_api_key_client(SummaryProvider::OpenAi, store, OPENAI_API_KEY_ACCOUNT).unwrap();
        assert!(client.resolve_service_target("gpt-4o-mini").await.is_ok());
        assert!(client
            .resolve_service_target("gemini-3-flash-preview")
            .await
            .is_err());
    }

    #[test]
    fn missing_credentials_identify_the_required_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(credential_store::EncryptedFileStore::new(dir.path()).unwrap());
        for (key, expected) in [
            (
                "claude-vertex",
                BuildSummarizerError::MissingVertexCredentials,
            ),
            ("openai", BuildSummarizerError::MissingApiKey),
        ] {
            assert_eq!(
                build_summarizer(key, store.clone(), None, None).err(),
                Some(expected)
            );
        }
        for key in ["claude-cli", "codex", "claude-oauth", "ollama"] {
            assert!(build_summarizer(key, store.clone(), None, None).is_ok());
        }
    }
}
