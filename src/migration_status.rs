//! Strict observations of the candidate VSS migration-status contract.
//! This module does not activate migration or assert deployed server support.

use crate::{receipt::ReceiptReader, LdkVssClient, VssClient, VssError};
use prost::{
    encoding::{decode_key, decode_varint, WireType},
    Message,
};
use std::collections::HashSet;

const STATUS_LIMIT: u32 = 64 * 1024;
const PROTOCOL: &str = "bitkit-migration-v1";

/// Physical identity reported by this authenticated status read. Equal store IDs
/// do not imply equal principals. No Debug/Serialize or persisted replay authority.
pub struct MigrationState {
    pub effective_principal: String,
    pub store_id: String,
    pub policy: MigrationPolicy,
}

pub enum MigrationPolicy {
    Legacy,
    Committed(CommittedMigration),
}

pub struct CommittedMigration {
    pub migration_id: String,
    pub proposal_sha256: [u8; 32],
    pub archive: Option<MigrationArchive>,
    pub initial_head: Option<MigrationObject>,
    pub objects: Vec<MigrationObject>,
}

pub struct MigrationArchive {
    pub key: String,
    /// None is an explicit protected-absence condition, not a failed read.
    pub ciphertext_sha256: Option<[u8; 32]>,
}

pub struct MigrationObject {
    pub key: String,
    pub ciphertext_sha256: [u8; 32],
}

impl VssClient {
    pub(crate) async fn read_migration_state(&self) -> Result<MigrationState, VssError> {
        read(&self.receipt_reader, &self.store_id).await
    }
}

impl LdkVssClient {
    pub(crate) async fn read_migration_state(&self) -> Result<MigrationState, VssError> {
        read(&self.receipt_reader, &self.store_id).await
    }
}

async fn read(reader: &ReceiptReader, store_id: &str) -> Result<MigrationState, VssError> {
    if !valid_identity(store_id) {
        return Err(invalid());
    }
    let request = wire::Request {
        store_id: store_id.to_owned(),
    }
    .encode_to_vec();
    let (status, body) = reader
        .post("getStoreMigrationState", request, STATUS_LIMIT)
        .await?;
    match status {
        reqwest::StatusCode::OK => decode(&body, store_id),
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
            Err(VssError::AuthError {
                error_details: "Migration status was not authorized".into(),
            })
        }
        // Even a typed missing-key 404 is not legacy policy.
        _ => Err(VssError::GetError {
            error_details: "Migration status is unavailable".into(),
        }),
    }
}

fn decode(bytes: &[u8], store_id: &str) -> Result<MigrationState, VssError> {
    validate_wire(bytes, true)?;
    let state = wire::State::decode(bytes).map_err(|_| invalid())?;
    if state.store_id != store_id
        || !valid_identity(&state.store_id)
        || !valid_identity(&state.effective_principal)
    {
        return Err(invalid());
    }
    let policy = match state.epoch {
        0 if state.required_protocol.is_empty()
            && state.migration_id.is_empty()
            && state.proposal_sha256.is_empty()
            && state.archive.is_none()
            && state.initial_head.is_none()
            && state.objects.is_empty() =>
        {
            MigrationPolicy::Legacy
        }
        1 if state.required_protocol == PROTOCOL && valid_uuid(&state.migration_id) => {
            MigrationPolicy::Committed(committed(&state)?)
        }
        _ => return Err(invalid()),
    };
    Ok(MigrationState {
        effective_principal: state.effective_principal,
        store_id: state.store_id,
        policy,
    })
}

fn committed(state: &wire::State) -> Result<CommittedMigration, VssError> {
    // Activation bounds bytes; existing legacy PostgreSQL identities are varchar(120).
    if state.store_id.len() > 120 || state.effective_principal.len() > 120 {
        return Err(invalid());
    }
    let archive = state
        .archive
        .as_ref()
        .map(|a| {
            valid_key(&a.key)?;
            Ok(MigrationArchive {
                key: a.key.clone(),
                ciphertext_sha256: a.ciphertext_sha256.as_deref().map(digest).transpose()?,
            })
        })
        .transpose()?;
    let initial_head = state.initial_head.as_ref().map(object).transpose()?;
    if archive
        .as_ref()
        .zip(initial_head.as_ref())
        .is_some_and(|(a, h)| a.key == h.key)
    {
        return Err(invalid());
    }
    let mut keys = HashSet::new();
    let mut objects = Vec::with_capacity(state.objects.len());
    for row in &state.objects {
        let row = object(row)?;
        if !keys.insert(row.key.clone()) || initial_head.as_ref().is_some_and(|h| h.key == row.key)
        {
            return Err(invalid());
        }
        // The server permits an archive to also be a referenced component. The
        // observations must agree on its existence and exact encrypted digest.
        if archive
            .as_ref()
            .is_some_and(|a| a.key == row.key && a.ciphertext_sha256 != Some(row.ciphertext_sha256))
        {
            return Err(invalid());
        }
        objects.push(row);
    }
    // An LDK-only tuple can omit archive and head. The all-tuples digest cannot be
    // recomputed from one status response; it is an observation, not proof of coherence.
    Ok(CommittedMigration {
        migration_id: state.migration_id.clone(),
        proposal_sha256: digest(&state.proposal_sha256)?,
        archive,
        initial_head,
        objects,
    })
}

fn object(row: &wire::Object) -> Result<MigrationObject, VssError> {
    valid_key(&row.key)?;
    Ok(MigrationObject {
        key: row.key.clone(),
        ciphertext_sha256: digest(&row.ciphertext_sha256)?,
    })
}
fn digest(value: &[u8]) -> Result<[u8; 32], VssError> {
    value.try_into().map_err(|_| invalid())
}
fn valid_identity(value: &str) -> bool {
    !value.is_empty() && value.chars().count() <= 120
}
fn valid_key(value: &str) -> Result<(), VssError> {
    if value.is_empty()
        || value.len() > 600
        || value == "global_version"
        || value == "vss_global_version"
    {
        return Err(invalid());
    }
    Ok(())
}
fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
            }
        })
}
fn invalid() -> VssError {
    VssError::InvalidData {
        error_details: "Invalid migration status".into(),
    }
}

/// Validate wire values before prost narrows integer types or merges duplicate
/// fields. Unknown fields fail closed until their semantics are supported.
fn validate_wire(mut bytes: &[u8], state: bool) -> Result<(), VssError> {
    let mut seen = 0u16;
    let mut objects = 0;
    while !bytes.is_empty() {
        let (tag, wire) = decode_key(&mut bytes).map_err(|_| invalid())?;
        if tag == 0 || tag > if state { 9 } else { 2 } {
            return Err(invalid());
        }
        if state && tag == 7 {
            objects += 1;
            if objects > 32 {
                return Err(invalid());
            }
        } else if seen & (1 << tag) != 0 {
            return Err(invalid());
        }
        seen |= 1 << tag;
        if state && tag == 1 {
            if wire != WireType::Varint || decode_varint(&mut bytes).map_err(|_| invalid())? > 1 {
                return Err(invalid());
            }
        } else {
            if wire != WireType::LengthDelimited {
                return Err(invalid());
            }
            let length = usize::try_from(decode_varint(&mut bytes).map_err(|_| invalid())?)
                .map_err(|_| invalid())?;
            if length > bytes.len() {
                return Err(invalid());
            }
            let (field, tail) = bytes.split_at(length);
            if state && (5..=7).contains(&tag) {
                validate_wire(field, false)?;
            }
            bytes = tail;
        }
    }
    Ok(())
}

// Pinned to the preserved VSS server candidate proto/vss.proto. These private
// messages intentionally add no UniFFI API or dependency on unpublished packages.
pub(crate) mod wire {
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Request {
        #[prost(string, tag = "1")]
        pub store_id: String,
    }
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Archive {
        #[prost(string, tag = "1")]
        pub key: String,
        #[prost(bytes = "vec", optional, tag = "2")]
        pub ciphertext_sha256: Option<Vec<u8>>,
    }
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Object {
        #[prost(string, tag = "1")]
        pub key: String,
        #[prost(bytes = "vec", tag = "2")]
        pub ciphertext_sha256: Vec<u8>,
    }
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct State {
        #[prost(int64, tag = "1")]
        pub epoch: i64,
        #[prost(string, tag = "2")]
        pub required_protocol: String,
        #[prost(string, tag = "3")]
        pub migration_id: String,
        #[prost(bytes = "vec", tag = "4")]
        pub proposal_sha256: Vec<u8>,
        #[prost(message, optional, tag = "5")]
        pub archive: Option<Archive>,
        #[prost(message, optional, tag = "6")]
        pub initial_head: Option<Object>,
        #[prost(message, repeated, tag = "7")]
        pub objects: Vec<Object>,
        #[prost(string, tag = "8")]
        pub effective_principal: String,
        #[prost(string, tag = "9")]
        pub store_id: String,
    }
}

#[cfg(test)]
mod tests;
