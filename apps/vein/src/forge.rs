use octocrab::Octocrab;
use std::env;

pub struct ForgeClient {
    pub inner: Octocrab,
}

impl ForgeClient {
    pub fn new() -> Result<Self, String> {
        let token = env::var("GITHUB_TOKEN").map_err(|_| "GITHUB_TOKEN not set".to_string())?;
        let instance = Octocrab::builder()
            .personal_token(token)
            .build()
            .map_err(|e| format!("Failed to build Octocrab: {}", e))?;

        Ok(Self { inner: instance })
    }

    pub async fn get_user_info(&self) -> Result<String, String> {
        match self.inner.current().user().await {
            Ok(user) => Ok(format!("Logged in as: {}", user.login)),
            Err(e) => Err(format!("Failed to fetch user info: {}", e)),
        }
    }

    pub async fn list_repos(&self) -> Result<Vec<String>, String> {
        self.inner
            .current()
            .list_repos_for_authenticated_user()
            .send()
            .await
            .map(|page| page.into_iter().map(|repo| repo.name).collect())
            .map_err(|e| format!("Failed to list repos: {}", e))
    }

    pub async fn get_file_content(&self, owner: &str, repo: &str, path: &str) -> Result<String, String> {
        let content_items = self.inner
            .repos(owner, repo)
            .get_content()
            .path(path)
            .send()
            .await
            .map_err(|e| format!("Failed to fetch file content: {}", e))?;

        // Octocrab's get_content returns a ContentItems struct which can contain multiple items (directory listing)
        // or a single file. For this method, we expect a single file.
        // We iterate and decode.

        if content_items.items.is_empty() {
            return Err("File not found or empty".to_string());
        }

        let first_item = &content_items.items[0];

        // Ensure it has content to decode
        if let Some(encoded_content) = &first_item.content {
             // Octocrab handles base64 decoding internally if we use the right helper,
             // but here we might get the raw base64 string with newlines.
             // simpler: ContentItems usually has a helper or we just use the raw bytes if available.
             // Actually, octocrab 0.38 models: repo::Content has `content`, `encoding` (usually "base64").
             // The `decoded_content()` method on `Content` is available in recent versions.

             // Let's try the safest manual decode to avoid version ambiguities if `decoded_content` is missing
             // or just use the `content` string (newlines stripped) and decode.
             let clean_b64 = encoded_content.replace("\n", "");
             use base64::Engine as _;
             let bytes = base64::engine::general_purpose::STANDARD.decode(&clean_b64).map_err(|e| format!("Base64 decode error: {}", e))?;
             String::from_utf8(bytes).map_err(|e| format!("UTF-8 decode error: {}", e))
        } else {
             Err("No content in file response".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation_without_token() {
        env::remove_var("GITHUB_TOKEN");
        let client = ForgeClient::new();
        assert!(client.is_err());
    }
}
