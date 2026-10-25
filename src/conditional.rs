use crate::{ConditionalKeyValue, ConditionalWriteError, KeyVersion, VssClient, VssError};
use prost::Message;
use std::{collections::HashSet, sync::Arc, time::Duration};
use vss_client_ng::{
    client::VssClient as ExternalClient,
    error::VssError as ExternalError,
    headers::VssHeaderProvider,
    types::{KeyValue, PutObjectRequest},
    util::retry::{ExponentialBackoffRetryPolicy, MaxAttemptsRetryPolicy, RetryPolicy},
};

pub(crate) type SingleAttempt =
    MaxAttemptsRetryPolicy<ExponentialBackoffRetryPolicy<ExternalError>>;

pub(crate) fn client(
    base_url: &str,
    headers: Arc<dyn VssHeaderProvider>,
) -> Result<ExternalClient<SingleAttempt>, VssError> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .map_err(|_| VssError::ConnectionError {
            error_details: "Cannot initialize conditional backup transport".into(),
        })?;
    let retry = ExponentialBackoffRetryPolicy::new(Duration::ZERO).with_max_attempts(1);
    Ok(ExternalClient::from_client_and_headers(
        base_url.to_owned(),
        http,
        retry,
        headers,
    ))
}

impl VssClient {
    /// See [`crate::vss_put_conditional`] for persistence and retry requirements.
    pub async fn put_conditional(
        &self,
        items: Vec<ConditionalKeyValue>,
    ) -> Result<Vec<KeyVersion>, ConditionalWriteError> {
        validate(&items)?;
        let request = self.conditional_request(&items);
        self.conditional_inner
            .put_object(&request)
            .await
            .map_err(classify)?;
        Ok(items
            .into_iter()
            .map(|item| KeyVersion {
                key: item.key,
                version: item.expected_version + 1,
            })
            .collect())
    }

    fn conditional_request(&self, items: &[ConditionalKeyValue]) -> PutObjectRequest {
        PutObjectRequest {
            store_id: self.store_id.clone(),
            global_version: None,
            transaction_items: items
                .iter()
                .map(|item| {
                    let key = self.build_key(&item.key);
                    let storable = self.storable_builder.build(
                        item.value.clone(),
                        item.expected_version + 1,
                        &self.app_data_encryption_key,
                        key.as_bytes(),
                    );
                    KeyValue {
                        key,
                        value: storable.encode_to_vec(),
                        version: item.expected_version,
                    }
                })
                .collect(),
            delete_items: vec![],
        }
    }
}

fn validate(items: &[ConditionalKeyValue]) -> Result<(), ConditionalWriteError> {
    let mut keys = HashSet::new();
    if items.is_empty() {
        return Err(invalid("At least one key is required"));
    }
    for item in items {
        if item.key.is_empty() || !keys.insert(&item.key) {
            return Err(invalid("Keys must be nonempty and unique"));
        }
        if item.expected_version < 0 || item.expected_version == i64::MAX {
            return Err(invalid(
                "Expected versions must be between zero and i64::MAX - 1",
            ));
        }
    }
    Ok(())
}

fn invalid(reason: &str) -> ConditionalWriteError {
    ConditionalWriteError::InvalidInput {
        reason: reason.into(),
    }
}

fn classify(error: ExternalError) -> ConditionalWriteError {
    match error {
        ExternalError::ConflictError(_) => ConditionalWriteError::Conflict,
        ExternalError::AuthError(_) | ExternalError::InvalidRequestError(_) => {
            ConditionalWriteError::Rejected
        }
        _ => ConditionalWriteError::OutcomeUnknown,
    }
}

#[cfg(test)]
mod tests;
