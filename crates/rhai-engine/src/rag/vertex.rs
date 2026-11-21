//! Google Cloud Vertex AI RAG Engine (Discovery Engine) integration.
//!
//! Uses the same OAuth2 credential as existing Vertex AI summarization.
//! The credential JSON must include the project_id for Discovery Engine API calls.

use std::sync::Arc;

use credential_store::CredentialStore;

use crate::SettingsProvider;

pub async fn search(
    query: &str,
    options: &rhai::Map,
    credential_store: &Arc<credential_store::FallbackCredentialStore>,
    settings: &dyn SettingsProvider,
) -> Result<rhai::Dynamic, String> {
    let data_store = get_opt_string(options, "data_store").unwrap_or_else(|| "default".to_string());
    let location = get_opt_string(options, "location").unwrap_or_else(|| "global".to_string());
    let max_results = options
        .get("max_results")
        .and_then(|v| v.as_int().ok())
        .unwrap_or(5) as usize;

    let credentials = load_vertex_credentials(credential_store, settings)?;
    let project_id = credentials.project_id.clone();

    let url = format!(
        "https://discoveryengine.googleapis.com/v1/projects/{project_id}/locations/{location}/dataStores/{data_store}/servingConfigs/default_search:search"
    );

    let body = serde_json::json!({
        "query": query,
        "pageSize": max_results,
        "queryExpansionSpec": { "condition": "AUTO" },
        "spellCorrectionSpec": { "mode": "AUTO" },
    });

    let token = get_access_token(&credentials).await?;
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {token}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Vertex RAG search failed: {e}"))?;

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("failed to parse response: {e}"))?;

    let mut results = rhai::Array::new();
    if let Some(results_arr) = json["results"].as_array() {
        for r in results_arr {
            let mut m = rhai::Map::new();
            if let Some(doc) = r["document"]["derivedStructData"].as_object() {
                m.insert(
                    "title".into(),
                    rhai::Dynamic::from(
                        doc.get("title")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                    ),
                );
                m.insert(
                    "snippet".into(),
                    rhai::Dynamic::from(
                        doc.get("snippets")
                            .and_then(|v| v.as_array())
                            .and_then(|a| a.first())
                            .and_then(|s| s["snippet"].as_str())
                            .unwrap_or("")
                            .to_string(),
                    ),
                );
                m.insert(
                    "link".into(),
                    rhai::Dynamic::from(
                        doc.get("link")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                    ),
                );
            }
            results.push(rhai::Dynamic::from_map(m));
        }
    }

    Ok(rhai::Dynamic::from_array(results))
}

pub async fn index(
    documents: &rhai::Dynamic,
    options: &rhai::Map,
    credential_store: &Arc<credential_store::FallbackCredentialStore>,
    settings: &dyn SettingsProvider,
) -> Result<rhai::Dynamic, String> {
    let data_store = get_opt_string(options, "data_store").unwrap_or_else(|| "default".to_string());
    let location = get_opt_string(options, "location").unwrap_or_else(|| "global".to_string());
    let branch = get_opt_string(options, "branch").unwrap_or_else(|| "default_branch".to_string());

    let credentials = load_vertex_credentials(credential_store, settings)?;
    let project_id = credentials.project_id.clone();

    let url = format!(
        "https://discoveryengine.googleapis.com/v1/projects/{project_id}/locations/{location}/dataStores/{data_store}/branches/{branch}/documents:import"
    );

    // Convert Rhai documents to Discovery Engine format
    let docs: Vec<serde_json::Value> =
        if let Some(arr) = documents.clone().try_cast::<rhai::Array>() {
            arr.iter()
                .map(|d| {
                    let text = d.to_string();
                    serde_json::json!({
                        "id": uuid::Uuid::new_v4().to_string(),
                        "jsonData": text,
                        "content": { "mimeType": "text/plain", "rawBytes": base64_encode(&text) },
                    })
                })
                .collect()
        } else {
            let text = documents.to_string();
            vec![serde_json::json!({
                "id": uuid::Uuid::new_v4().to_string(),
                "jsonData": text,
                "content": { "mimeType": "text/plain", "rawBytes": base64_encode(&text) },
            })]
        };

    let body = serde_json::json!({
        "reconciliationMode": "INCREMENTAL",
        "inlineSource": { "documents": docs },
    });

    let token = get_access_token(&credentials).await?;
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {token}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Vertex RAG index failed: {e}"))?;

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("failed to parse response: {e}"))?;
    let count = json["errorSamples"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);

    let mut m = rhai::Map::new();
    m.insert(
        "indexed".into(),
        rhai::Dynamic::from((docs.len() as i64) - (count as i64)),
    );
    m.insert("errors".into(), rhai::Dynamic::from(count as i64));
    Ok(rhai::Dynamic::from_map(m))
}

struct VertexCredentials {
    project_id: String,
    client_email: String,
    private_key: String,
}

fn load_vertex_credentials(
    credential_store: &Arc<credential_store::FallbackCredentialStore>,
    _settings: &dyn SettingsProvider,
) -> Result<VertexCredentials, String> {
    let raw = credential_store
        .load(
            summarize::CREDENTIAL_SERVICE,
            summarize::CLAUDE_VERTEX_CREDENTIALS_ACCOUNT,
        )
        .or_else(|_| {
            credential_store.load(
                summarize::CREDENTIAL_SERVICE,
                summarize::GEMINI_VERTEX_CREDENTIALS_ACCOUNT,
            )
        })
        .map_err(|_| "Vertex AI credentials not configured".to_string())?;

    let json: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("invalid credentials JSON: {e}"))?;
    Ok(VertexCredentials {
        project_id: json["project_id"].as_str().unwrap_or("").to_string(),
        client_email: json["client_email"].as_str().unwrap_or("").to_string(),
        private_key: json["private_key"].as_str().unwrap_or("").to_string(),
    })
}

async fn get_access_token(credentials: &VertexCredentials) -> Result<String, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let jwt = build_token_request_jwt(credentials, now)?;

    let client = reqwest::Client::new();
    let resp = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", &jwt),
        ])
        .send()
        .await
        .map_err(|e| format!("token request failed: {e}"))?;

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("token response parse failed: {e}"))?;
    json["access_token"]
        .as_str()
        .map(String::from)
        .ok_or("no access_token in response".to_string())
}

/// Signs the service-account JWT that is exchanged for an OAuth access token
/// (RS256, valid for one hour from `now`).
fn build_token_request_jwt(credentials: &VertexCredentials, now: u64) -> Result<String, String> {
    let expiry = now + 3600;

    jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
        &serde_json::json!({
            "iss": credentials.client_email,
            "scope": "https://www.googleapis.com/auth/cloud-platform",
            "aud": "https://oauth2.googleapis.com/token",
            "exp": expiry,
            "iat": now,
        }),
        &jsonwebtoken::EncodingKey::from_rsa_pem(credentials.private_key.as_bytes())
            .map_err(|e| format!("invalid private key: {e}"))?,
    )
    .map_err(|e| format!("JWT encoding failed: {e}"))
}

fn get_opt_string(map: &rhai::Map, key: &str) -> Option<String> {
    map.get(key).and_then(|v| v.clone().try_cast::<String>())
}

fn base64_encode(s: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(s.as_bytes())
}
#[cfg(test)]
mod tests {
    use super::*;
    use aws_lc_rs::encoding::AsDer;
    use base64::Engine;

    /// A fresh PKCS#8 PEM, the same shape as a service-account key's
    /// `private_key`. Generated per run so no key is committed.
    fn throwaway_rsa_pem() -> String {
        let key = aws_lc_rs::rsa::KeyPair::generate(aws_lc_rs::rsa::KeySize::Rsa2048).unwrap();
        let der: aws_lc_rs::encoding::Pkcs8V1Der = key.as_der().unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(der.as_ref());
        let body: Vec<&str> = b64
            .as_bytes()
            .chunks(64)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect();
        format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
            body.join("\n")
        )
    }

    #[test]
    fn token_request_jwt_is_signed_with_rs256() {
        let credentials = VertexCredentials {
            project_id: "p".to_string(),
            client_email: "sa@p.iam.gserviceaccount.com".to_string(),
            private_key: throwaway_rsa_pem(),
        };
        let jwt =
            build_token_request_jwt(&credentials, 1_700_000_000).expect("JWT should be signed");

        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[0])
            .unwrap();
        let header: serde_json::Value = serde_json::from_slice(&header).unwrap();
        assert_eq!(header["alg"], "RS256");
        let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(parts[1])
            .unwrap();
        let claims: serde_json::Value = serde_json::from_slice(&claims).unwrap();
        assert_eq!(claims["iss"], "sa@p.iam.gserviceaccount.com");
        assert_eq!(claims["exp"], 1_700_003_600);
        assert!(!parts[2].is_empty());
    }
}
