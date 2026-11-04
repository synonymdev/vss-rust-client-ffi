//! Immutable proposal content, never migration execution or remote authority.

use super::*;
use crate::{MigrationArchive, MigrationObject, MigrationPolicy};
use bitcoin::hashes::{sha256, Hash};
use prost::Message;

mod canonical;
mod reconciliation;
pub use reconciliation::{MigrationReconciliation, MigrationReconciliationError};

const MAX_HEAD_BYTES: usize = 10 * 1024 * 1024;

/// Plaintext for a new app head. Its schema and coherent capture remain app duties.
pub struct MigrationHeadInput<'a> {
    pub key: &'a str,
    pub value: &'a [u8],
}

/// Both native domains must be observed, even when they share a physical tuple.
/// `archive` must be the original app WALLET observation, including explicit absence.
/// Components must be present app observations already safe to make immutable.
/// Mutable LDK operational keys are never components. No principal, digest or proof is supplied
/// by the caller. This input cannot establish running-node or legacy-reader exclusion.
pub struct MigrationPreparation<'a> {
    pub migration_id: &'a str,
    pub app_status: &'a MigrationObservation,
    pub ldk_status: &'a MigrationObservation,
    pub archive: &'a BackupReadObservation,
    pub initial_head: MigrationHeadInput<'a>,
    pub components: &'a [&'a BackupReadObservation],
}

#[derive(Debug, thiserror::Error)]
pub enum MigrationPreparationError {
    #[error(transparent)]
    Ownership(#[from] BackupReadError),
    #[error("A newer migration status read invalidated this observation")]
    StalePolicy,
    #[error("The observed store is already migrated")]
    AlreadyMigrated,
    #[error("Invalid migration preparation: {0}")]
    InvalidInput(&'static str),
}

/// Frozen ciphertext and expectations belonging to one original in-memory owner.
/// No serialization, Debug, credentials, automatic replay or activation method.
/// Access through the owning session; a new status read invalidates access even if
/// that read fails or returns the same policy. Revocation cannot retract copied data.
pub struct PreparedMigration {
    owner: Arc<watch::Sender<bool>>,
    revisions: [Arc<()>; 2],
    content: PreparedMigrationContent,
}

pub struct PreparedMigrationContent {
    migration_id: String,
    proposal_sha256: [u8; 32],
    stores: Vec<PreparedMigrationStore>,
}

/// One unique physical principal/store pair in canonical UTF-8 order.
/// The domain list retains which original providers must participate. Two domains
/// are coalesced only when both reported the exact same principal AND store.
pub struct PreparedMigrationStore {
    domains: Vec<BackupDomain>,
    effective_principal: String,
    store_id: String,
    archive: Option<MigrationArchive>,
    initial_head: Option<PreparedMigrationHead>,
    objects: Vec<MigrationObject>,
}

pub struct PreparedMigrationHead {
    key: String,
    encrypted_value: Vec<u8>,
}

impl BackupReadSession {
    /// Encrypts the initial head once under this session's original app key/AAD.
    /// Performs no I/O. An archive stays at its original encrypted key and is never
    /// decrypted/re-encrypted here. A separately prepared proposal has new ciphertext;
    /// only repeated access to THIS result preserves an exact attempt. Supply a fresh
    /// UUID for each independent preparation; this API has no durable UUID/replay registry.
    ///
    /// These checks are local observations, not a coherent snapshot or permission to
    /// write. Future execution requires fresh proofs checked atomically against the
    /// original tuples and exact envelope conditions by an enforcing server, plus
    /// application lifecycle drains and retained uncertain-outcome reconciliation.
    pub fn prepare_migration(
        &self,
        input: MigrationPreparation<'_>,
    ) -> Result<PreparedMigration, MigrationPreparationError> {
        self.ensure_active()?;
        canonical::validate_id(input.migration_id)?;
        let app = self.preparation_status(input.app_status, BackupDomain::App)?;
        let ldk = self.preparation_status(input.ldk_status, BackupDomain::Ldk)?;
        let mut stores = physical_stores(app, ldk)?;
        let app_index = store_index(&stores, BackupDomain::App);
        stores[app_index].archive = Some(self.prepare_archive(input.archive)?);
        self.prepare_components(&mut stores, input.components)?;
        stores[app_index].initial_head = Some(self.prepare_head(input.initial_head)?);
        canonical::validate_objects(&mut stores)?;
        let content = PreparedMigrationContent {
            migration_id: input.migration_id.to_owned(),
            proposal_sha256: canonical::digest(input.migration_id, &stores),
            stores,
        };
        let prepared = PreparedMigration {
            owner: Arc::clone(&self.revoked),
            revisions: [
                Arc::clone(&input.app_status.revision),
                Arc::clone(&input.ldk_status.revision),
            ],
            content,
        };
        self.prepared_migration(&prepared)?;
        Ok(prepared)
    }

    /// Checks original ownership, revocation and that neither status has been
    /// superseded at this instant. The returned content is not execution authority.
    pub fn prepared_migration<'a>(
        &self,
        prepared: &'a PreparedMigration,
    ) -> Result<&'a PreparedMigrationContent, MigrationPreparationError> {
        if !Arc::ptr_eq(&self.revoked, &prepared.owner) {
            return Err(BackupReadError::DifferentOwner.into());
        }
        self.ensure_active()?;
        for (current, captured) in self.migration_revisions.iter().zip(&prepared.revisions) {
            if !Arc::ptr_eq(&current.borrow(), captured) {
                return Err(MigrationPreparationError::StalePolicy);
            }
        }
        Ok(&prepared.content)
    }

    fn preparation_status<'a>(
        &self,
        observation: &'a MigrationObservation,
        domain: BackupDomain,
    ) -> Result<&'a MigrationState, MigrationPreparationError> {
        let state = self.migration_state(observation)?;
        if observation.domain != domain {
            return Err(invalid("Status credential domain does not match"));
        }
        if !Arc::ptr_eq(
            &self.migration_revisions[domain.index()].borrow(),
            &observation.revision,
        ) {
            return Err(MigrationPreparationError::StalePolicy);
        }
        if !matches!(state.policy, MigrationPolicy::Legacy) {
            return Err(MigrationPreparationError::AlreadyMigrated);
        }
        Ok(state)
    }

    fn prepare_archive(
        &self,
        observation: &BackupReadObservation,
    ) -> Result<MigrationArchive, MigrationPreparationError> {
        let receipt = self.receipt(observation)?;
        if !matches!(&observation.target, BackupReadTarget::App { key } if key == "WALLET") {
            return Err(invalid("Archive must be the original app WALLET"));
        }
        Ok(MigrationArchive {
            key: self.app.build_key("WALLET"),
            ciphertext_sha256: receipt.map(receipt_digest).transpose()?,
        })
    }

    fn prepare_components(
        &self,
        stores: &mut [PreparedMigrationStore],
        observations: &[&BackupReadObservation],
    ) -> Result<(), MigrationPreparationError> {
        if observations.len() > 32 {
            return Err(invalid("Too many component observations"));
        }
        for observation in observations {
            let receipt = self
                .receipt(observation)?
                .ok_or(invalid("A component is absent"))?;
            if !matches!(observation.target, BackupReadTarget::App { .. }) {
                return Err(invalid(
                    "LDK operational objects cannot be immutable components",
                ));
            }
            let store = &mut stores[store_index(stores, BackupDomain::App)];
            if receipt.store_id != store.store_id {
                return Err(invalid("Component store does not match observed policy"));
            }
            store.objects.push(MigrationObject {
                key: receipt.storage_key.clone(),
                ciphertext_sha256: receipt_digest(receipt)?,
            });
        }
        Ok(())
    }

    fn prepare_head(
        &self,
        input: MigrationHeadInput<'_>,
    ) -> Result<PreparedMigrationHead, MigrationPreparationError> {
        if input.key.is_empty()
            || input.key.len() > 600
            || input.value.is_empty()
            || input.value.len() > MAX_HEAD_BYTES
        {
            return Err(invalid(
                "Initial head must have a key and bounded nonempty content",
            ));
        }
        let key = self.app.build_key(input.key);
        canonical::valid_key(&key)?;
        let value = self
            .app
            .storable_builder
            .build(
                input.value.to_vec(),
                1,
                &self.app.app_data_encryption_key,
                key.as_bytes(),
            )
            .encode_to_vec();
        if value.len() > MAX_HEAD_BYTES {
            return Err(invalid("Encrypted initial head exceeds server limit"));
        }
        Ok(PreparedMigrationHead {
            key,
            encrypted_value: value,
        })
    }
}

fn physical_stores(
    app: &MigrationState,
    ldk: &MigrationState,
) -> Result<Vec<PreparedMigrationStore>, MigrationPreparationError> {
    let mut stores: Vec<PreparedMigrationStore> = Vec::new();
    for (domain, state) in [(BackupDomain::App, app), (BackupDomain::Ldk, ldk)] {
        if state.effective_principal.is_empty()
            || state.effective_principal.len() > 120
            || state.store_id.is_empty()
            || state.store_id.len() > 120
        {
            return Err(invalid(
                "Migration identities must fit the server's byte limits",
            ));
        }
        if let Some(store) = stores.iter_mut().find(|s| {
            s.effective_principal == state.effective_principal && s.store_id == state.store_id
        }) {
            store.domains.push(domain);
        } else {
            stores.push(PreparedMigrationStore {
                domains: vec![domain],
                effective_principal: state.effective_principal.clone(),
                store_id: state.store_id.clone(),
                archive: None,
                initial_head: None,
                objects: Vec::new(),
            });
        }
    }
    stores.sort_by(|a, b| {
        (&a.effective_principal, &a.store_id).cmp(&(&b.effective_principal, &b.store_id))
    });
    Ok(stores)
}

fn store_index(stores: &[PreparedMigrationStore], domain: BackupDomain) -> usize {
    stores
        .iter()
        .position(|s| s.domains.contains(&domain))
        .expect("Both original domains were retained")
}

fn receipt_digest(receipt: &VssReadReceipt) -> Result<[u8; 32], MigrationPreparationError> {
    receipt
        .envelope_sha256
        .parse::<sha256::Hash>()
        .map(|h| h.to_byte_array())
        .map_err(|_| invalid("Invalid observed ciphertext digest"))
}

fn invalid(reason: &'static str) -> MigrationPreparationError {
    MigrationPreparationError::InvalidInput(reason)
}

impl PreparedMigrationContent {
    pub fn migration_id(&self) -> &str {
        &self.migration_id
    }
    pub fn proposal_sha256(&self) -> &[u8; 32] {
        &self.proposal_sha256
    }
    pub fn stores(&self) -> &[PreparedMigrationStore] {
        &self.stores
    }
}

impl PreparedMigrationStore {
    pub fn domains(&self) -> &[BackupDomain] {
        &self.domains
    }
    pub fn effective_principal(&self) -> &str {
        &self.effective_principal
    }
    pub fn store_id(&self) -> &str {
        &self.store_id
    }
    pub fn archive(&self) -> Option<&MigrationArchive> {
        self.archive.as_ref()
    }
    pub fn initial_head(&self) -> Option<&PreparedMigrationHead> {
        self.initial_head.as_ref()
    }
    pub fn objects(&self) -> &[MigrationObject] {
        &self.objects
    }
}

impl PreparedMigrationHead {
    pub fn key(&self) -> &str {
        &self.key
    }
    /// Exact envelope for an expected-version-zero insert, with authenticated version one.
    pub fn encrypted_value(&self) -> &[u8] {
        &self.encrypted_value
    }
}
