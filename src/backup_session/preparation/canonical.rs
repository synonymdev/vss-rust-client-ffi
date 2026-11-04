use super::*;
use bitcoin::hashes::HashEngine;

pub(super) fn validate_id(id: &str) -> Result<(), MigrationPreparationError> {
    if id.len() != 36
        || !id.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
            }
        })
    {
        return Err(invalid("Migration ID must be a canonical lowercase UUID"));
    }
    Ok(())
}

pub(super) fn validate_objects(
    stores: &mut [PreparedMigrationStore],
) -> Result<(), MigrationPreparationError> {
    for store in stores {
        if store.objects.len() > 32 {
            return Err(invalid("Too many components in one physical store"));
        }
        if let Some(archive) = &store.archive {
            valid_key(&archive.key)?;
        }
        if let Some(head) = &store.initial_head {
            valid_key(&head.key)?;
            if store.archive.as_ref().is_some_and(|a| a.key == head.key) {
                return Err(invalid("Initial head must not replace the archive"));
            }
        }
        store.objects.sort_by(|a, b| a.key.cmp(&b.key));
        if store.objects.windows(2).any(|p| p[0].key == p[1].key) {
            return Err(invalid("Duplicate component key in a physical store"));
        }
        for object in &store.objects {
            valid_key(&object.key)?;
            if store
                .initial_head
                .as_ref()
                .is_some_and(|h| h.key == object.key)
            {
                return Err(invalid("Initial head conflicts with a component"));
            }
            if store.archive.as_ref().is_some_and(|a| {
                a.key == object.key && a.ciphertext_sha256 != Some(object.ciphertext_sha256)
            }) {
                return Err(invalid("Archive and component observations conflict"));
            }
        }
    }
    Ok(())
}

pub(super) fn valid_key(key: &str) -> Result<(), MigrationPreparationError> {
    if key.is_empty() || key.len() > 600 || key == "global_version" || key == "vss_global_version" {
        return Err(invalid("Invalid encrypted storage key"));
    }
    Ok(())
}

// Candidate server's canonical encoding; no proofs or local domain labels are hashed.
// Callers have already sorted unique physical tuples and component keys.
pub(super) fn digest(id: &str, stores: &[PreparedMigrationStore]) -> [u8; 32] {
    let mut hash = sha256::Hash::engine();
    hash.input(b"vss-store-migration-v1\0");
    field(&mut hash, id.as_bytes());
    hash.input(&(stores.len() as u32).to_be_bytes());
    for store in stores {
        field(&mut hash, store.effective_principal.as_bytes());
        field(&mut hash, store.store_id.as_bytes());
        hash.input(&0_i64.to_be_bytes());
        hash.input(&[u8::from(store.archive.is_some())]);
        if let Some(archive) = &store.archive {
            field(&mut hash, archive.key.as_bytes());
            hash.input(&[u8::from(archive.ciphertext_sha256.is_some())]);
            if let Some(digest) = &archive.ciphertext_sha256 {
                field(&mut hash, digest);
            }
        }
        hash.input(&[u8::from(store.initial_head.is_some())]);
        if let Some(head) = &store.initial_head {
            field(&mut hash, head.key.as_bytes());
            field(&mut hash, &head.encrypted_value);
        }
        hash.input(&(store.objects.len() as u32).to_be_bytes());
        for object in &store.objects {
            field(&mut hash, object.key.as_bytes());
            field(&mut hash, &object.ciphertext_sha256);
        }
    }
    sha256::Hash::from_engine(hash).to_byte_array()
}

fn field(hash: &mut sha256::HashEngine, bytes: &[u8]) {
    hash.input(&(bytes.len() as u32).to_be_bytes());
    hash.input(bytes);
}

#[cfg(test)]
mod tests;
