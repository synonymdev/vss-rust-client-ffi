//! Read-only, in-memory ownership across app and LDK backup observations.

use crate::{LdkNamespace, LdkVssClient, MigrationState, VssClient, VssError, VssReadReceipt};
use std::sync::Arc;
use tokio::sync::watch;

mod preparation;
pub use preparation::{
    MigrationHeadInput, MigrationPreparation, MigrationPreparationError, MigrationReconciliation,
    MigrationReconciliationError, PreparedMigration, PreparedMigrationContent,
    PreparedMigrationHead, PreparedMigrationStore,
};

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

/// Credential/derivation domain, independent of textual store identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackupDomain {
    App,
    Ldk,
}

/// Server-reported policy bound to one original session and credential domain.
/// Not serializable or Debug; it carries physical storage identity, not authority.
pub struct MigrationObservation {
    owner: Arc<watch::Sender<bool>>,
    revision: Arc<()>,
    domain: BackupDomain,
    state: MigrationState,
}

impl MigrationObservation {
    pub fn domain(&self) -> BackupDomain {
        self.domain
    }
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
    migration_revisions: [watch::Sender<Arc<()>>; 2],
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
            migration_revisions: std::array::from_fn(|_| watch::channel(Arc::new(())).0),
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

    /// One read of the candidate VSS migration-status contract. Unsupported routes,
    /// policy errors and missing/malformed tuples are errors, never legacy absence.
    /// The ten-second deadline includes credentials; the storage body is capped at 64 KiB.
    pub async fn read_migration_state(
        &self,
        domain: BackupDomain,
    ) -> Result<MigrationObservation, BackupReadError> {
        self.migration_observation(domain, async {
            match domain {
                BackupDomain::App => self.app.read_migration_state().await,
                BackupDomain::Ldk => self.ldk.read_migration_state().await,
            }
        })
        .await
    }

    async fn migration_observation(
        &self,
        domain: BackupDomain,
        read: impl std::future::Future<Output = Result<MigrationState, VssError>>,
    ) -> Result<MigrationObservation, BackupReadError> {
        let mut cancellation = self.revoked.subscribe();
        self.ensure_active()?;
        // Pending, failed and abandoned reads invalidate earlier preparation.
        let revision = Arc::new(());
        self.migration_revisions[domain.index()].send_replace(Arc::clone(&revision));
        let state = tokio::select! {
            biased;
            _ = cancellation.changed() => return Err(BackupReadError::Revoked),
            result = read => result,
        };
        self.ensure_active()?;
        Ok(MigrationObservation {
            owner: Arc::clone(&self.revoked),
            revision,
            domain,
            state: state?,
        })
    }

    /// Local observation access only. A future mutation must freshly and atomically
    /// bind its authorization to the observed principal/store on an enforcing server.
    pub fn migration_state<'a>(
        &self,
        observation: &'a MigrationObservation,
    ) -> Result<&'a MigrationState, BackupReadError> {
        if !Arc::ptr_eq(&self.revoked, &observation.owner) {
            return Err(BackupReadError::DifferentOwner);
        }
        self.ensure_active()?;
        Ok(&observation.state)
    }

    fn ensure_active(&self) -> Result<(), BackupReadError> {
        if *self.revoked.borrow() {
            return Err(BackupReadError::Revoked);
        }
        Ok(())
    }
}

impl BackupDomain {
    fn index(self) -> usize {
        match self {
            Self::App => 0,
            Self::Ldk => 1,
        }
    }
}

impl Drop for BackupReadSession {
    fn drop(&mut self) {
        self.revoke();
    }
}

#[cfg(test)]
mod tests;
