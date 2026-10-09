use crate::{model::Event, store::private_permissions, Error, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use chacha20poly1305::{
    aead::{Aead, Payload},
    ChaCha20Poly1305, KeyInit, Nonce,
};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs::OpenOptions, io::Write, path::Path};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    version: u32,
    nonce: String,
    ciphertext: String,
}

pub fn generate_key(path: &Path) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    let mut key = [0u8; 32];
    OsRng.fill_bytes(&mut key);
    file.write_all(&key)?;
    file.sync_all()?;
    private_permissions(path, false)?;
    Ok(())
}
pub fn read_key(path: &Path) -> Result<[u8; 32]> {
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.is_file() || meta.len() != 32 {
        return Err(Error::new(
            "invalid_key",
            "Key must be a regular file containing 32 random bytes",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(Error::new(
                "unsafe_key_permissions",
                "Key file must be accessible only to its owner (chmod 600)",
            ));
        }
    }
    std::fs::read(path)?
        .try_into()
        .map_err(|_| Error::new("invalid_key", "Key must contain 32 bytes"))
}
pub fn key_id(key: &[u8; 32]) -> String {
    format!("{:x}", Sha256::digest(key))
}
fn aad(key: &[u8; 32], revision: &str) -> Vec<u8> {
    format!("continuo-events/v1/{}/{revision}", key_id(key)).into_bytes()
}
pub fn seal(key: &[u8; 32], event: &Event) -> Result<Vec<u8>> {
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let plaintext = serde_json::to_vec(event)?;
    let ciphertext = ChaCha20Poly1305::new(key.into())
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &plaintext,
                aad: &aad(key, &event.revision),
            },
        )
        .map_err(|_| Error::new("encryption_failed", "Unable to encrypt event"))?;
    Ok(serde_json::to_vec(&Envelope {
        version: 1,
        nonce: STANDARD.encode(nonce),
        ciphertext: STANDARD.encode(ciphertext),
    })?)
}
pub fn open(key: &[u8; 32], revision: &str, bytes: &[u8]) -> Result<Event> {
    let env: Envelope = serde_json::from_slice(bytes)?;
    if env.version != 1 {
        return Err(Error::new(
            "unsupported_schema",
            "Unsupported encryption envelope",
        ));
    }
    let nonce = STANDARD
        .decode(env.nonce)
        .map_err(|_| Error::new("invalid_ciphertext", "Invalid nonce"))?;
    if nonce.len() != 12 {
        return Err(Error::new("invalid_ciphertext", "Invalid nonce length"));
    }
    let ciphertext = STANDARD
        .decode(env.ciphertext)
        .map_err(|_| Error::new("invalid_ciphertext", "Invalid ciphertext"))?;
    let plaintext = ChaCha20Poly1305::new(key.into())
        .decrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &ciphertext,
                aad: &aad(key, revision),
            },
        )
        .map_err(|_| {
            Error::new(
                "decryption_failed",
                "Key is incorrect or synchronized data was tampered with",
            )
        })?;
    let event: Event = serde_json::from_slice(&plaintext)?;
    event.validate()?;
    if event.revision != revision {
        return Err(Error::new(
            "invalid_event",
            "Event revision does not match its envelope",
        ));
    }
    Ok(event)
}
