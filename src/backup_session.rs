//! Read-only, in-memory ownership across app and LDK backup observations.

use crate::{LdkNamespace, LdkVssClient, VssClient, VssError, VssReadReceipt};
use std::sync::Arc;
use tokio::sync::watch;

/// One immutable storage/auth configuration for both native derivation domains.
/// Equal store IDs do not establish equal physical server principals.
pub struct BackupReadConfig {
    pub base_url: String,
    pub store_id: String,
    pub lnurl_auth_server_url: String,
}

/// The original logical address, retained even for an explicit missing-key result.
pub enum BackupReadTarget {
    App {
        key: String,
    },
    /// The existing ldk-node V1 namespace. Absence does not rule out older V0 storage.
    Ldk {
        key: String,
        namespace: LdkNamespace,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum BackupReadError {
    #[error("The backup read session was revoked")]
    Revoked,
    #[error("The observation belongs to a different backup read session")]
    DifferentOwner,
    #[error(transparent)]
    Read(#[from] VssError),
}

/// An observation bound to one native session. Deliberately not serializable or
/// Debug: it can contain a complete plaintext wallet and is not remote authority.
pub struct BackupReadObservation {
    owner: Arc<watch::Sender<bool>>,
    target: BackupReadTarget,
    receipt: Option<VssReadReceipt>,
}

impl BackupReadObservation {
    pub fn target(&self) -> &BackupReadTarget {
        &self.target
    }
}

/// Captures both clients directly from one wallet seed, never from global slots.
/// Revocation is permanent. It rejects new reads and discards pending results,
/// but cannot preempt every already-admitted transport poll or revoke a previously
/// borrowed or copied plaintext value.
/// No method writes, authorizes migration, or proves multi-object coherence.
pub struct BackupReadSession {
    app: VssClient,
    ldk: LdkVssClient,
    revoked: Arc<watch::Sender<bool>>,
}

impl BackupReadSession {
    pub async fn new(config: BackupReadConfig, seed: [u8; 64]) -> Result<Self, VssError> {
        let app = VssClient::new_with_lnurl_auth(
            config.base_url.clone(),
            config.store_id.clone(),
            seed,
            config.lnurl_auth_server_url.clone(),
        )
        .await?;
        let ldk = LdkVssClient::new_with_lnurl_auth(
            config.base_url,
            config.store_id,
            seed,
            config.lnurl_auth_server_url,
        )
        .await?;
        Ok(Self {
            app,
            ldk,
            revoked: Arc::new(watch::channel(false).0),
        })
    }

    pub fn revoke(&self) {
        self.revoked.send_replace(true);
    }

    /// Each operation has the receipt reader's ten-second deadline and storage byte cap.
    /// The inherited credential provider's separate responses are not byte-capped.
    /// Dropping this future abandons only this read; it never schedules a retry.
    pub async fn read(
        &self,
        target: BackupReadTarget,
        max_response_bytes: u32,
    ) -> Result<BackupReadObservation, BackupReadError> {
        let mut cancellation = self.revoked.subscribe();
        self.ensure_active()?;
        let read = async {
            match &target {
                BackupReadTarget::App { key } => {
                    self.app
                        .get_with_receipt(key.clone(), max_response_bytes)
                        .await
                }
                BackupReadTarget::Ldk { key, namespace } => {
                    self.ldk
                        .get_with_receipt(key.clone(), namespace, max_response_bytes)
                        .await
                }
            }
        };
        let receipt = tokio::select! {
            biased;
            _ = cancellation.changed() => return Err(BackupReadError::Revoked),
            result = read => result,
        };
        self.ensure_active()?;
        let receipt = receipt?;
        Ok(BackupReadObservation {
            owner: Arc::clone(&self.revoked),
            target,
            receipt,
        })
    }

    /// Checks in-memory ownership and revocation at this instant. The returned
    /// reference is an observation, not admission for a later write: callers must
    /// separately enforce lifecycle drains, freshness and mutation preconditions.
    pub fn receipt<'a>(
        &self,
        observation: &'a BackupReadObservation,
    ) -> Result<Option<&'a VssReadReceipt>, BackupReadError> {
        if !Arc::ptr_eq(&self.revoked, &observation.owner) {
            return Err(BackupReadError::DifferentOwner);
        }
        self.ensure_active()?;
        Ok(observation.receipt.as_ref())
    }

    fn ensure_active(&self) -> Result<(), BackupReadError> {
        if *self.revoked.borrow() {
            return Err(BackupReadError::Revoked);
        }
        Ok(())
    }
}

impl Drop for BackupReadSession {
    fn drop(&mut self) {
        self.revoke();
    }
}

#[cfg(test)]
mod tests;
