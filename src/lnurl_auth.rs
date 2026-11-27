//! LNURL-auth JWT header provider that signs for a fixed key domain.
//!
//! Port of vss-client-ng 0.4.1 `src/headers/lnurl_auth_jwt.rs`. Upstream derives the
//! LUD-05 linking key from the domain of the LNURL the auth server returns. This provider
//! derives it from a configured key domain instead and sends the signed callback to the
//! host of the configured auth URL, so the auth server can move hosts while wallets keep
//! the same linking key (the JWT `sub` that VSS stores data under).

use crate::errors::VssError;
use crate::implementation::{VSS_HARDENED_CHILD_INDEX, VSS_LNURL_AUTH_HARDENED_CHILD_INDEX};
use async_trait::async_trait;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use bitcoin::bip32::{ChildNumber, DerivationPath, Xpriv};
use bitcoin::hashes::hex::FromHex;
use bitcoin::hashes::{sha256, Hash, HashEngine, Hmac, HmacEngine};
use bitcoin::secp256k1::{Message, Secp256k1, SignOnly};
use bitcoin::{Network, PrivateKey};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};
use url::Url;
use vss_client_ng::headers::{LnurlAuthToJwtProvider, VssHeaderProvider, VssHeaderProviderError};

// Derivation index of the hashing private key as defined by LUD-05.
const HASHING_DERIVATION_INDEX: u32 = 0;
// The JWT token will be refreshed by the given amount before its expiry.
const EXPIRY_BUFFER: Duration = Duration::from_secs(60);
const K1_QUERY_PARAM: &str = "k1";
const SIG_QUERY_PARAM: &str = "sig";
const KEY_QUERY_PARAM: &str = "key";
const AUTHORIZATION: &str = "Authorization";

/// Builds the LNURL-auth header provider: vss-client-ng's own when `key_domain` is `None`,
/// otherwise one that signs for `key_domain` and calls back on the auth URL's host.
pub(crate) fn lnurl_auth_header_provider(
    parent_key: Xpriv,
    url: String,
    key_domain: Option<String>,
) -> Result<Arc<dyn VssHeaderProvider>, VssError> {
    let provider_error = |e: VssHeaderProviderError| VssError::ConnectionError {
        error_details: format!("Failed to create LNURL-auth provider: {}", e),
    };
    match key_domain {
        None => Ok(Arc::new(
            LnurlAuthToJwtProvider::new(parent_key, url, HashMap::new()).map_err(provider_error)?,
        )),
        Some(key_domain) => Ok(Arc::new(
            KeyDomainLnurlAuthProvider::new(parent_key, url, key_domain).map_err(provider_error)?,
        )),
    }
}

/// LNURL-auth parent key of the LDK client: `m/877'/138'` from the full 64-byte seed,
/// as ldk-node's `build_with_vss_store` derives it.
pub(crate) fn ldk_lnurl_auth_xprv(seed: &[u8; 64]) -> Result<Xpriv, VssError> {
    let secp = Secp256k1::new();
    Xpriv::new_master(Network::Bitcoin, seed)
        .and_then(|master| {
            master.derive_priv(
                &secp,
                &[
                    ChildNumber::Hardened {
                        index: VSS_HARDENED_CHILD_INDEX,
                    },
                    ChildNumber::Hardened {
                        index: VSS_LNURL_AUTH_HARDENED_CHILD_INDEX,
                    },
                ],
            )
        })
        .map_err(|e| VssError::ConnectionError {
            error_details: format!("Failed to derive LNURL-auth key: {}", e),
        })
}

#[derive(Debug, Clone)]
struct JwtToken {
    token_str: String,
    expiry: Option<SystemTime>,
}

impl JwtToken {
    fn is_expired(&self) -> bool {
        self.expiry
            .and_then(|expiry| {
                SystemTime::now()
                    .checked_add(EXPIRY_BUFFER)
                    .map(|now_with_buffer| now_with_buffer > expiry)
            })
            .unwrap_or(false)
    }
}

struct KeyDomainLnurlAuthProvider {
    engine: Secp256k1<SignOnly>,
    parent_key: Xpriv,
    url: Url,
    key_domain: String,
    client: reqwest::Client,
    cached_jwt_token: RwLock<Option<JwtToken>>,
}

impl KeyDomainLnurlAuthProvider {
    fn new(
        parent_key: Xpriv,
        url: String,
        key_domain: String,
    ) -> Result<Self, VssHeaderProviderError> {
        let url = Url::parse(&url).map_err(|e| VssHeaderProviderError::InvalidData {
            error: format!("invalid LNURL-auth server URL: {}", e),
        })?;
        // Canonicalize as `Url::domain()` does for the upstream key domain.
        let key_domain = match url::Host::parse(&key_domain) {
            Ok(url::Host::Domain(domain)) => domain,
            _ => {
                return Err(VssHeaderProviderError::InvalidData {
                    error: format!(
                        "invalid LNURL-auth key domain: {}",
                        key_domain.escape_debug()
                    ),
                })
            }
        };
        let client = reqwest::Client::builder().build().map_err(|e| {
            VssHeaderProviderError::RequestError {
                error: e.to_string(),
            }
        })?;
        Ok(Self {
            engine: Secp256k1::signing_only(),
            parent_key,
            url,
            key_domain,
            client,
            cached_jwt_token: RwLock::new(None),
        })
    }

    async fn fetch_jwt_token(&self) -> Result<JwtToken, VssHeaderProviderError> {
        let request_error = |e: reqwest::Error| VssHeaderProviderError::RequestError {
            error: e.to_string(),
        };
        let lnurl_str = self
            .client
            .get(self.url.clone())
            .send()
            .await
            .map_err(request_error)?
            .text()
            .await
            .map_err(request_error)?;

        let signed_lnurl = sign_lnurl(
            &self.engine,
            &self.parent_key,
            &lnurl_str,
            &self.key_domain,
            &self.url,
        )?;
        let lnurl_auth_response: LnurlAuthResponse = self
            .client
            .get(&signed_lnurl)
            .send()
            .await
            .map_err(request_error)?
            .json()
            .await
            .map_err(request_error)?;

        let untrusted_token = match lnurl_auth_response {
            LnurlAuthResponse {
                token: Some(token), ..
            } => token,
            LnurlAuthResponse {
                reason: Some(reason),
                ..
            } => {
                return Err(VssHeaderProviderError::AuthorizationError {
                    error: format!("LNURL Auth failed, reason is: {}", reason.escape_debug()),
                });
            }
            _ => {
                return Err(VssHeaderProviderError::InvalidData {
                    error: "LNURL Auth response did not contain a token nor an error".to_string(),
                });
            }
        };
        parse_jwt_token(untrusted_token)
    }

    async fn get_jwt_token(&self) -> Result<String, VssHeaderProviderError> {
        let cached_token_str = {
            let jwt_token = self.cached_jwt_token.read().unwrap();
            jwt_token
                .as_ref()
                .filter(|t| !t.is_expired())
                .map(|t| t.token_str.clone())
        };
        if let Some(token_str) = cached_token_str {
            return Ok(token_str);
        }
        let jwt_token = self.fetch_jwt_token().await?;
        *self.cached_jwt_token.write().unwrap() = Some(jwt_token.clone());
        Ok(jwt_token.token_str)
    }
}

#[async_trait]
impl VssHeaderProvider for KeyDomainLnurlAuthProvider {
    async fn get_headers(
        &self,
        _request: &[u8],
    ) -> Result<HashMap<String, String>, VssHeaderProviderError> {
        let jwt_token = self.get_jwt_token().await?;
        Ok(HashMap::from([(
            AUTHORIZATION.to_string(),
            format!("Bearer {}", jwt_token),
        )]))
    }
}

fn hashing_key(
    engine: &Secp256k1<SignOnly>,
    parent_key: &Xpriv,
) -> Result<PrivateKey, VssHeaderProviderError> {
    let hashing_child_number =
        ChildNumber::from_normal_idx(HASHING_DERIVATION_INDEX).map_err(internal_error)?;
    parent_key
        .derive_priv(engine, &[hashing_child_number])
        .map(|xpriv| xpriv.to_priv())
        .map_err(internal_error)
}

fn linking_key_path(hashing_key: &PrivateKey, domain_name: &str) -> DerivationPath {
    let mut engine = HmacEngine::<sha256::Hash>::new(&hashing_key.inner[..]);
    engine.input(domain_name.as_bytes());
    let result = Hmac::<sha256::Hash>::from_engine(engine).to_byte_array();
    // unwrap safety: We take 4-byte chunks, so TryInto for [u8; 4] never fails.
    let children = result
        .chunks_exact(4)
        .take(4)
        .map(|i| u32::from_be_bytes(i.try_into().unwrap()))
        .map(ChildNumber::from);
    DerivationPath::from_iter(children)
}

/// Signs `lnurl_str` with the linking key for `key_domain` and points the callback at
/// `auth_url`'s scheme, host and port, keeping its path and query.
fn sign_lnurl(
    engine: &Secp256k1<SignOnly>,
    parent_key: &Xpriv,
    lnurl_str: &str,
    key_domain: &str,
    auth_url: &Url,
) -> Result<String, VssHeaderProviderError> {
    let invalid_lnurl = || VssHeaderProviderError::InvalidData {
        error: format!("invalid lnurl: {}", lnurl_str.escape_debug()),
    };
    let mut lnurl = Url::parse(lnurl_str).map_err(|_| invalid_lnurl())?;
    let k1_str = lnurl
        .query_pairs()
        .find(|(k, _)| k == K1_QUERY_PARAM)
        .ok_or_else(invalid_lnurl)?
        .1
        .to_string();
    let k1: [u8; 32] = FromHex::from_hex(&k1_str).map_err(|_| invalid_lnurl())?;

    let hashing_private_key = hashing_key(engine, parent_key)?;
    let linking_key_path = linking_key_path(&hashing_private_key, key_domain);
    let linking_private_key = parent_key
        .derive_priv(engine, &linking_key_path)
        .map_err(internal_error)?
        .to_priv();
    let linking_public_key = linking_private_key.public_key(engine);
    let message = Message::from_digest(k1);
    let sig = engine.sign_ecdsa(&message, &linking_private_key.inner);

    lnurl
        .set_scheme(auth_url.scheme())
        .map_err(|_| invalid_lnurl())?;
    lnurl
        .set_host(auth_url.host_str())
        .map_err(|_| invalid_lnurl())?;
    lnurl
        .set_port(auth_url.port())
        .map_err(|_| invalid_lnurl())?;
    lnurl
        .query_pairs_mut()
        .append_pair(SIG_QUERY_PARAM, &sig.serialize_der().to_string())
        .append_pair(KEY_QUERY_PARAM, &linking_public_key.to_string());
    Ok(lnurl.to_string())
}

fn internal_error(e: bitcoin::bip32::Error) -> VssHeaderProviderError {
    VssHeaderProviderError::InternalError {
        error: e.to_string(),
    }
}

#[derive(Deserialize, Debug, Clone)]
struct LnurlAuthResponse {
    reason: Option<String>,
    token: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
struct ExpiryClaim {
    #[serde(rename = "exp")]
    expiry_secs: Option<u64>,
}

fn parse_jwt_token(jwt_token: String) -> Result<JwtToken, VssHeaderProviderError> {
    let parts: Vec<&str> = jwt_token.split('.').collect();
    let invalid = || VssHeaderProviderError::InvalidData {
        error: format!("invalid JWT token: {}", jwt_token.escape_debug()),
    };
    if parts.len() != 3 {
        return Err(invalid());
    }
    let _ = URL_SAFE_NO_PAD.decode(parts[0]).map_err(|_| invalid())?;
    let bytes = URL_SAFE_NO_PAD.decode(parts[1]).map_err(|_| invalid())?;
    let _ = URL_SAFE_NO_PAD.decode(parts[2]).map_err(|_| invalid())?;
    let claim: ExpiryClaim = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    let expiry = claim
        .expiry_secs
        .and_then(|e| SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(e)));
    Ok(JwtToken {
        token_str: jwt_token,
        expiry,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const K1: &str = "e2af6254a8df433264fa23f67eb8188635d15ce883e8fc020989d5f82ae6f11e";

    fn upstream_test_parent_key() -> Xpriv {
        let bytes: [u8; 32] =
            FromHex::from_hex("abababababababababababababababababababababababababababababababab")
                .unwrap();
        Xpriv::new_master(Network::Testnet, &bytes).unwrap()
    }

    fn sign(lnurl: &str, key_domain: &str, auth_url: &str) -> String {
        sign_lnurl(
            &Secp256k1::signing_only(),
            &upstream_test_parent_key(),
            lnurl,
            key_domain,
            &Url::parse(auth_url).unwrap(),
        )
        .unwrap()
    }

    // vss-client-ng 0.4.1's `sign_lnurl` with the same parent key, for the `bitkit.to`
    // LNURL the mainnet auth server returns.
    #[test]
    fn signs_for_bitkit_to_on_api_bitkit_to() {
        let signed = sign(
            &format!("https://bitkit.to/lnurl_auth/auth?tag=login&k1={K1}&action=login"),
            "bitkit.to",
            "https://api.bitkit.to/lnurl_auth/auth",
        );
        assert_eq!(
            signed,
            format!("https://api.bitkit.to/lnurl_auth/auth?tag=login&k1={K1}&action=login&sig=3045022100b8924431a127bbad6ebe3ce7f9f1894907c1087395306ef1f6a2b7cdcb63fc9f022067c20224274f743c70b8465619369d06742ccc7a88f3d2def14d79dd4c1ed346&key=03a758e690253eb7dace3ee0f351da6fb54f15873cc767dfc85d616e2d9f498986"),
        );
    }

    // The key domain decides the key, whatever host the returned LNURL names.
    #[test]
    fn signs_for_key_domain_when_lnurl_host_differs() {
        let signed = sign(
            &format!("https://api.bitkit.to/lnurl_auth/auth?tag=login&k1={K1}&action=login"),
            "bitkit.to",
            "https://api.bitkit.to/lnurl_auth/auth",
        );
        assert!(signed
            .ends_with("&key=03a758e690253eb7dace3ee0f351da6fb54f15873cc767dfc85d616e2d9f498986"));
    }

    #[test]
    fn canonicalizes_key_domain() {
        let new = |key_domain: &str| {
            KeyDomainLnurlAuthProvider::new(
                upstream_test_parent_key(),
                "https://api.bitkit.to/lnurl_auth/auth".to_string(),
                key_domain.to_string(),
            )
        };
        assert_eq!(new("BITKIT.TO").unwrap().key_domain, "bitkit.to");
        assert!(new("").is_err());
        assert!(new("https://bitkit.to").is_err());
    }

    // vss-client-ng 0.4.1's published `test_sign_lnurl` vector, signed for `example.com`
    // while the callback moves to another host.
    #[test]
    fn matches_upstream_vector_on_another_host() {
        let signed = sign(
            &format!("https://example.com/path?tag=login&k1={K1}"),
            "example.com",
            "http://127.0.0.1:8080/lnurl",
        );
        assert_eq!(
            signed,
            format!("http://127.0.0.1:8080/path?tag=login&k1={K1}&sig=3045022100a75df468de452e618edb8030016eb0894204655c7d93ece1be007fcf36843522022048bc2f00a0a5a30601d274b49cfaf9ef4c76176e5401d0dfb195f5d6ab8ab4c4&key=02d9eb1b467517d685e3b5439082c14bb1a2c9ae672df4d9046d208c193a5846e0"),
        );
    }

    // Serves the mainnet challenge shape on loopback: the callback must reach the auth host,
    // carry the bitkit.to key, and the JWT must be reused while unexpired.
    #[tokio::test]
    async fn fetches_and_caches_jwt_via_auth_host() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;
        use std::sync::{Arc as StdArc, Mutex};

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let payload = URL_SAFE_NO_PAD.encode(br#"{"sub":"x","exp":4102444800}"#);
        let jwt = format!("e30.{payload}.c2ln");
        let paths = StdArc::new(Mutex::new(Vec::new()));
        let server_paths = StdArc::clone(&paths);
        let server_jwt = jwt.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut request_line = String::new();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                reader.read_line(&mut request_line).unwrap();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    server_paths
                        .lock()
                        .unwrap()
                        .push(line.trim_end().to_ascii_lowercase());
                }
                let path = request_line.split(' ').nth(1).unwrap().to_string();
                server_paths.lock().unwrap().push(path.clone());
                let body = if path.contains("sig=") {
                    format!(r#"{{"token":"{server_jwt}"}}"#)
                } else {
                    format!("https://bitkit.to/lnurl_auth/auth?tag=login&k1={K1}&action=login")
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            }
        });

        let provider = KeyDomainLnurlAuthProvider::new(
            upstream_test_parent_key(),
            format!("http://{addr}/lnurl_auth/auth"),
            "bitkit.to".to_string(),
        )
        .unwrap();
        for _ in 0..2 {
            let headers = provider.get_headers(&[]).await.unwrap();
            assert_eq!(headers.get("Authorization"), Some(&format!("Bearer {jwt}")));
        }

        let paths = paths.lock().unwrap();
        let requests: Vec<&String> = paths.iter().filter(|p| p.starts_with('/')).collect();
        assert_eq!(requests.len(), 2, "the second call reuses the cached JWT");
        assert_eq!(requests[0], "/lnurl_auth/auth");
        assert!(requests[1].starts_with(&format!(
            "/lnurl_auth/auth?tag=login&k1={K1}&action=login&sig="
        )));
        assert!(requests[1]
            .ends_with("&key=03a758e690253eb7dace3ee0f351da6fb54f15873cc767dfc85d616e2d9f498986"));
        assert!(paths.contains(&format!("host: {addr}")));
    }
}
