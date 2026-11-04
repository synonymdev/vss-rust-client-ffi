//! Strict, read-only snapshots of the encrypted objects stored by VSS.

use crate::{VssClient, VssError, VssReadReceipt};
use bitcoin::hashes::{sha256, Hash};
use prost::{
    encoding::{decode_key, decode_varint, skip_field, DecodeContext, WireType},
    Message,
};
use std::{sync::Arc, time::Duration};
use vss_client_ng::{
    headers::{VssHeaderProvider, VssHeaderProviderError},
    types::{ErrorCode, ErrorResponse, GetObjectRequest, GetObjectResponse, Storable},
};

const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RESPONSE_BYTES: u32 = 64 * 1024 * 1024;

pub(crate) struct ReceiptReader {
    http: reqwest::Client,
    url: String,
    headers: Arc<dyn VssHeaderProvider>,
}

impl ReceiptReader {
    pub(crate) fn new(
        base_url: &str,
        headers: Arc<dyn VssHeaderProvider>,
    ) -> Result<Self, VssError> {
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .map_err(|_| network_error())?;
        Ok(Self {
            http,
            url: format!("{}/getObject", base_url.trim_end_matches('/')),
            headers,
        })
    }

    async fn read(
        &self,
        request: GetObjectRequest,
        limit: u32,
    ) -> Result<Option<vss_client_ng::types::KeyValue>, VssError> {
        if limit == 0 || limit > MAX_RESPONSE_BYTES {
            return Err(invalid(
                "Response limit must be between 1 and 67108864 bytes",
            ));
        }
        tokio::time::timeout(TIMEOUT, self.request(request, limit as usize))
            .await
            .map_err(|_| network_error())?
    }

    async fn request(
        &self,
        request: GetObjectRequest,
        limit: usize,
    ) -> Result<Option<vss_client_ng::types::KeyValue>, VssError> {
        let body = request.encode_to_vec();
        let headers = self
            .headers
            .get_headers(&body)
            .await
            .map_err(credential_error)?;
        let mut request = self
            .http
            .post(&self.url)
            .header("Content-Type", "application/octet-stream");
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let response = request
            .body(body)
            .send()
            .await
            .map_err(|_| network_error())?;
        let status = response.status();
        let bytes = bounded_body(response, limit).await?;
        decode_response(status, &bytes)
    }
}

impl VssClient {
    /// Reads one app object without following redirects or retrying the storage request.
    /// See [`crate::vss_get_with_receipt`] for the receipt's limits.
    pub async fn get_with_receipt(
        &self,
        key: String,
        max_response_bytes: u32,
    ) -> Result<Option<VssReadReceipt>, VssError> {
        if key.is_empty() {
            return Err(invalid("The key must not be empty"));
        }
        let storage_key = self.build_key(&key);
        let request = GetObjectRequest {
            store_id: self.store_id.clone(),
            key: storage_key.clone(),
        };
        let Some(stored) = self
            .receipt_reader
            .read(request, max_response_bytes)
            .await?
        else {
            return Ok(None);
        };
        if stored.key != storage_key || stored.version <= 0 {
            return Err(invalid("The response key or server version is invalid"));
        }
        // Hash before decoding: protobuf re-encoding can discard unknown fields.
        let envelope_sha256 = sha256::Hash::hash(&stored.value).to_string();
        let storable = Storable::decode(stored.value.as_slice())
            .map_err(|_| invalid("Cannot decode the stored envelope"))?;
        if !storable
            .encryption_metadata
            .as_ref()
            .is_some_and(|metadata| metadata.cipher_format == "ChaCha20Poly1305")
        {
            return Err(invalid("Unsupported encrypted backup format"));
        }
        let (value, embedded_version) = self
            .storable_builder
            .deconstruct(
                storable,
                &self.app_data_encryption_key,
                storage_key.as_bytes(),
            )
            .map_err(|_| invalid("Cannot authenticate the stored envelope"))?;
        if embedded_version != -1 && embedded_version != stored.version {
            return Err(invalid(
                "The encrypted version does not match the server version",
            ));
        }
        Ok(Some(VssReadReceipt {
            key,
            value,
            store_id: self.store_id.clone(),
            storage_key,
            envelope_sha256,
            version: stored.version,
        }))
    }
}

async fn bounded_body(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, VssError> {
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return Err(invalid(
            "The backup response exceeds the requested byte limit",
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| network_error())? {
        if chunk.len() > limit - bytes.len() {
            return Err(invalid(
                "The backup response exceeds the requested byte limit",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn decode_response(
    status: reqwest::StatusCode,
    bytes: &[u8],
) -> Result<Option<vss_client_ng::types::KeyValue>, VssError> {
    if status == reqwest::StatusCode::NOT_FOUND && is_missing_key_response(bytes)? {
        return Ok(None);
    }
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(VssError::AuthError {
            error_details: "Backup read was not authorized".into(),
        });
    }
    if status != reqwest::StatusCode::OK {
        return Err(VssError::GetError {
            error_details: "Backup read did not return an object".into(),
        });
    }
    GetObjectResponse::decode(bytes)
        .map_err(|_| invalid("Invalid backup response"))?
        .value
        .map(Some)
        .ok_or_else(|| invalid("A successful backup response omitted its object"))
}

fn is_missing_key_response(mut bytes: &[u8]) -> Result<bool, VssError> {
    let malformed = || invalid("Invalid missing-key response");
    // Validate the known string field too, but do not trust the enum's narrowing
    // conversion from a wire u64 to i32 when deciding that backup data is absent.
    ErrorResponse::decode(bytes).map_err(|_| malformed())?;
    let mut code = None;
    while !bytes.is_empty() {
        let (tag, wire) = decode_key(&mut bytes).map_err(|_| malformed())?;
        if tag == 1 {
            if wire != WireType::Varint || code.is_some() {
                return Err(malformed());
            }
            code = Some(decode_varint(&mut bytes).map_err(|_| malformed())?);
        } else {
            skip_field(wire, tag, &mut bytes, DecodeContext::default()).map_err(|_| malformed())?;
        }
    }
    Ok(code == Some(ErrorCode::NoSuchKeyException as u64))
}

fn invalid(details: &str) -> VssError {
    VssError::InvalidData {
        error_details: details.into(),
    }
}

fn network_error() -> VssError {
    VssError::NetworkError {
        error_details: "Backup receipt transport failed or timed out".into(),
    }
}

fn credential_error(error: VssHeaderProviderError) -> VssError {
    match error {
        VssHeaderProviderError::AuthorizationError { .. } => VssError::AuthError {
            error_details: "Backup read credentials were not authorized".into(),
        },
        VssHeaderProviderError::RequestError { .. } => network_error(),
        _ => invalid("Cannot obtain valid backup read credentials"),
    }
}

#[cfg(test)]
mod tests;
