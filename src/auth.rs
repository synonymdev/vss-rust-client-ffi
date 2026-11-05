use async_trait::async_trait;
use std::{collections::HashMap, time::Duration};
use vss_client_ng::headers::{LnurlAuthToJwtProvider, VssHeaderProvider, VssHeaderProviderError};

const AUTH_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) struct AuthHeaderProvider(pub(crate) LnurlAuthToJwtProvider);

#[async_trait]
impl VssHeaderProvider for AuthHeaderProvider {
    async fn get_headers(
        &self,
        request: &[u8],
    ) -> Result<HashMap<String, String>, VssHeaderProviderError> {
        // The VSS request timeout starts after headers; bound both auth legs and their bodies.
        // Await directly so expiry drops this attempt before any VSS request can be sent.
        tokio::time::timeout(AUTH_TIMEOUT, self.0.get_headers(request))
            .await
            .map_err(|_| VssHeaderProviderError::RequestError {
                error: "LNURL authentication timed out".into(),
            })?
    }
}

#[cfg(test)]
mod tests;
