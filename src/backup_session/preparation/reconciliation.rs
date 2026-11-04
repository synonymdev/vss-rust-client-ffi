//! Read-only comparison of a retained attempt with fresh original-provider receipts.

use super::*;
use crate::CommittedMigration;

/// Observations from two sequential reads, never an atomic snapshot or permission
/// to activate/replay a migration. Even `ObservedUncommitted` may already be stale.
#[derive(Debug, PartialEq, Eq)]
pub enum MigrationReconciliation {
    ObservedCommitted,
    ObservedUncommitted,
}

#[derive(Debug, thiserror::Error)]
pub enum MigrationReconciliationError {
    #[error(transparent)]
    Ownership(#[from] BackupReadError),
    #[error("Migration status could not be read through the original {domain:?} provider")]
    Read {
        domain: BackupDomain,
        #[source]
        source: BackupReadError,
    },
    #[error("The original {0:?} provider now identifies a different participant")]
    ParticipantChanged(BackupDomain),
    #[error("The {0:?} receipt does not match the retained migration attempt")]
    ReceiptMismatch(BackupDomain),
    #[error("The original participants report mixed migration outcomes")]
    MixedOutcome,
    #[error("A newer status read superseded reconciliation")]
    SupersededObservation,
}

impl BackupReadSession {
    /// Reads each original credential domain once, including coalesced domains.
    /// Each read obtains fresh credentials with its captured original signer and
    /// includes authentication in the ten-second deadline. No storage retry,
    /// redirect, upload, deletion, activation or current-head read occurs here.
    /// The inherited LNURL authentication GETs retain their redirect/body behavior.
    ///
    /// The opaque attempt must belong to this still-active in-memory owner. Its
    /// preparation may already be stale: this method only compares immutable
    /// content, and never restores preparation access. It supplies no persisted
    /// UUID adoption or restart authority. Revocation discards pending results;
    /// dropping this future abandons its reads without scheduling continuation.
    pub async fn reconcile_migration(
        &self,
        prepared: &PreparedMigration,
    ) -> Result<MigrationReconciliation, MigrationReconciliationError> {
        if !Arc::ptr_eq(&self.revoked, &prepared.owner) {
            return Err(BackupReadError::DifferentOwner.into());
        }
        self.ensure_active()?;
        let app = self.reconciliation_observation(BackupDomain::App).await?;
        let ldk = self.reconciliation_observation(BackupDomain::Ldk).await?;
        let app_committed = matches_receipt(&prepared.content, &app)?;
        let ldk_committed = matches_receipt(&prepared.content, &ldk)?;
        self.ensure_reconciliation_current(&[app, ldk])?;
        match (app_committed, ldk_committed) {
            (true, true) => Ok(MigrationReconciliation::ObservedCommitted),
            (false, false) => Ok(MigrationReconciliation::ObservedUncommitted),
            _ => Err(MigrationReconciliationError::MixedOutcome),
        }
    }

    async fn reconciliation_observation(
        &self,
        domain: BackupDomain,
    ) -> Result<MigrationObservation, MigrationReconciliationError> {
        self.migration_observation(domain, async {
            match domain {
                BackupDomain::App => self.app.read_fresh_migration_state().await,
                BackupDomain::Ldk => self.ldk.read_fresh_migration_state().await,
            }
        })
        .await
        .map_err(|source| {
            if matches!(source, BackupReadError::Revoked) {
                MigrationReconciliationError::Ownership(source)
            } else {
                MigrationReconciliationError::Read { domain, source }
            }
        })
    }

    fn ensure_reconciliation_current(
        &self,
        observations: &[MigrationObservation; 2],
    ) -> Result<(), MigrationReconciliationError> {
        self.ensure_active()?;
        for observation in observations {
            if !Arc::ptr_eq(
                &self.migration_revisions[observation.domain.index()].borrow(),
                &observation.revision,
            ) {
                return Err(MigrationReconciliationError::SupersededObservation);
            }
        }
        Ok(())
    }
}

fn matches_receipt(
    prepared: &PreparedMigrationContent,
    observation: &MigrationObservation,
) -> Result<bool, MigrationReconciliationError> {
    let domain = observation.domain;
    let expected = &prepared.stores[store_index(&prepared.stores, domain)];
    let actual = &observation.state;
    if actual.effective_principal != expected.effective_principal
        || actual.store_id != expected.store_id
    {
        return Err(MigrationReconciliationError::ParticipantChanged(domain));
    }
    match &actual.policy {
        MigrationPolicy::Legacy => Ok(false),
        MigrationPolicy::Committed(receipt) if receipt_matches(prepared, expected, receipt) => {
            Ok(true)
        }
        MigrationPolicy::Committed(_) => Err(MigrationReconciliationError::ReceiptMismatch(domain)),
    }
}

fn receipt_matches(
    prepared: &PreparedMigrationContent,
    expected: &PreparedMigrationStore,
    actual: &CommittedMigration,
) -> bool {
    actual.migration_id == prepared.migration_id
        && actual.proposal_sha256 == prepared.proposal_sha256
        && archives_match(expected.archive.as_ref(), actual.archive.as_ref())
        && heads_match(expected.initial_head.as_ref(), actual.initial_head.as_ref())
        && objects_match(&expected.objects, &actual.objects)
}

fn archives_match(expected: Option<&MigrationArchive>, actual: Option<&MigrationArchive>) -> bool {
    match (expected, actual) {
        (None, None) => true,
        (Some(expected), Some(actual)) => {
            expected.key == actual.key && expected.ciphertext_sha256 == actual.ciphertext_sha256
        }
        _ => false,
    }
}

fn heads_match(expected: Option<&PreparedMigrationHead>, actual: Option<&MigrationObject>) -> bool {
    match (expected, actual) {
        (None, None) => true,
        (Some(expected), Some(actual)) => {
            expected.key == actual.key
                && sha256::Hash::hash(&expected.encrypted_value).to_byte_array()
                    == actual.ciphertext_sha256
        }
        _ => false,
    }
}

fn objects_match(expected: &[MigrationObject], actual: &[MigrationObject]) -> bool {
    // Both sets were validated as unique. Wire order is not part of the receipt.
    expected.len() == actual.len()
        && expected.iter().all(|object| {
            actual.iter().any(|candidate| {
                object.key == candidate.key
                    && object.ciphertext_sha256 == candidate.ciphertext_sha256
            })
        })
}
