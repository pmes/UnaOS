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
