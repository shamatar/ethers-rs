// Code adapted from: https://github.com/tomusdrw/rust-web3/blob/master/src/api/accounts.rs
use crate::{
    types::{Address, PublicKey, H256},
    utils::hash_message,
};

use rustc_hex::{FromHex, ToHex};
use serde::{Deserialize, Serialize};
use std::{convert::TryFrom, fmt, str::FromStr};

use thiserror::Error;

use elliptic_curve::consts::U32;
use generic_array::GenericArray;
use k256::ecdsa::{
    recoverable::{Id as RecoveryId, Signature as RecoverableSignature},
    Error as K256SignatureError, Signature as K256Signature,
};
use k256::EncodedPoint as K256PublicKey;

/// An error involving a signature.
#[derive(Debug, Error)]
pub enum SignatureError {
    /// Invalid length, secp256k1 signatures are 65 bytes
    #[error("invalid signature length, got {0}, expected 65")]
    InvalidLength(usize),
    /// When parsing a signature from string to hex
    #[error(transparent)]
    DecodingError(#[from] rustc_hex::FromHexError),
    /// Thrown when signature verification failed (i.e. when the address that
    /// produced the signature did not match the expected address)
    #[error("Signature verification failed. Expected {0}, got {0}")]
    VerificationError(Address, Address),
    /// Internal error during signature recovery
    #[error(transparent)]
    K256Error(#[from] K256SignatureError),
    /// Error in recovering public key from signature
    #[error("Public key recovery error")]
    RecoveryError,
}

/// Recovery message data.
///
/// The message data can either be a binary message that is first hashed
/// according to EIP-191 and then recovered based on the signature or a
/// precomputed hash.
#[derive(Clone, Debug, PartialEq)]
pub enum RecoveryMessage {
    /// Message bytes
    Data(Vec<u8>),
    /// Message hash
    Hash(H256),
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
/// An ECDSA signature
pub struct Signature {
    /// R value
    pub r: H256,
    /// S Value
    pub s: H256,
    /// V value in 'Electrum' notation.
    pub v: u64,
}

impl fmt::Display for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sig = <[u8; 65]>::from(self);
        write!(f, "{}", sig.to_hex::<String>())
    }
}

impl Signature {
    /// Verifies that signature on `message` was produced by `address`
    pub fn verify<M, A>(&self, message: M, address: A) -> Result<(), SignatureError>
    where
        M: Into<RecoveryMessage>,
        A: Into<Address>,
    {
        let address = address.into();
        let recovered = self.recover(message)?;
        if recovered != address {
            return Err(SignatureError::VerificationError(address, recovered));
        }

        Ok(())
    }

    /// Recovers the Ethereum address which was used to sign the given message.
    ///
    /// Recovery signature data uses 'Electrum' notation, this means the `v`
    /// value is expected to be either `27` or `28`.
    pub fn recover<M>(&self, message: M) -> Result<Address, SignatureError>
    where
        M: Into<RecoveryMessage>,
    {
        let message = message.into();
        let message_hash = match message {
            RecoveryMessage::Data(ref message) => hash_message(message),
            RecoveryMessage::Hash(hash) => hash,
        };

        let (recoverable_sig, _recovery_id) = self.as_signature()?;
        let verify_key =
            recoverable_sig.recover_verify_key_from_digest_bytes(message_hash.as_ref().into())?;

        let uncompressed_pub_key = K256PublicKey::from(&verify_key).decompress();
        if uncompressed_pub_key.is_some().into() {
            let pub_key: K256PublicKey = K256PublicKey::from(uncompressed_pub_key.unwrap());
            Ok(PublicKey::from(pub_key).into())
        } else {
            Err(SignatureError::RecoveryError)
        }
    }

    /// Retrieves the recovery signature.
    fn as_signature(&self) -> Result<(RecoverableSignature, RecoveryId), SignatureError> {
        let recovery_id = self.recovery_id()?;
        let signature = {
            let gar: &GenericArray<u8, U32> = GenericArray::from_slice(self.r.as_bytes());
            let gas: &GenericArray<u8, U32> = GenericArray::from_slice(self.s.as_bytes());
            let sig = K256Signature::from_scalars(*gar, *gas)?;
            RecoverableSignature::new(&sig, recovery_id)?
        };

        Ok((signature, recovery_id))
    }

    /// Retrieve the recovery ID.
    fn recovery_id(&self) -> Result<RecoveryId, SignatureError> {
        let standard_v = normalize_recovery_id(self.v);
        Ok(RecoveryId::new(standard_v)?)
    }

    /// Copies and serializes `self` into a new `Vec` with the recovery id included
    pub fn to_vec(&self) -> Vec<u8> {
        self.into()
    }
}

fn normalize_recovery_id(v: u64) -> u8 {
    match v {
        0 => 0,
        1 => 1,
        27 => 0,
        28 => 1,
        v if v >= 35 => ((v - 1) % 2) as _,
        _ => 4,
    }
}

impl<'a> TryFrom<&'a [u8]> for Signature {
    type Error = SignatureError;

    /// Parses a raw signature which is expected to be 65 bytes long where
    /// the first 32 bytes is the `r` value, the second 32 bytes the `s` value
    /// and the final byte is the `v` value in 'Electrum' notation.
    fn try_from(bytes: &'a [u8]) -> Result<Self, Self::Error> {
        if bytes.len() != 65 {
            return Err(SignatureError::InvalidLength(bytes.len()));
        }

        let v = bytes[64];
        let r = H256::from_slice(&bytes[0..32]);
        let s = H256::from_slice(&bytes[32..64]);

        Ok(Signature { r, s, v: v.into() })
    }
}

impl FromStr for Signature {
    type Err = SignatureError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = s.from_hex::<Vec<u8>>()?;
        Signature::try_from(&bytes[..])
    }
}

impl From<&Signature> for [u8; 65] {
    fn from(src: &Signature) -> [u8; 65] {
        let mut sig = [0u8; 65];
        sig[..32].copy_from_slice(src.r.as_bytes());
        sig[32..64].copy_from_slice(src.s.as_bytes());
        // TODO: What if we try to serialize a signature where
        // the `v` is not normalized?
        sig[64] = src.v as u8;
        sig
    }
}

impl From<Signature> for [u8; 65] {
    fn from(src: Signature) -> [u8; 65] {
        <[u8; 65]>::from(&src)
    }
}

impl From<&Signature> for Vec<u8> {
    fn from(src: &Signature) -> Vec<u8> {
        <[u8; 65]>::from(src).to_vec()
    }
}

impl From<Signature> for Vec<u8> {
    fn from(src: Signature) -> Vec<u8> {
        <[u8; 65]>::from(&src).to_vec()
    }
}

impl From<&[u8]> for RecoveryMessage {
    fn from(s: &[u8]) -> Self {
        s.to_owned().into()
    }
}

impl From<Vec<u8>> for RecoveryMessage {
    fn from(s: Vec<u8>) -> Self {
        RecoveryMessage::Data(s)
    }
}

impl From<&str> for RecoveryMessage {
    fn from(s: &str) -> Self {
        s.as_bytes().to_owned().into()
    }
}

impl From<String> for RecoveryMessage {
    fn from(s: String) -> Self {
        RecoveryMessage::Data(s.into_bytes())
    }
}

impl From<[u8; 32]> for RecoveryMessage {
    fn from(hash: [u8; 32]) -> Self {
        H256(hash).into()
    }
}

impl From<H256> for RecoveryMessage {
    fn from(hash: H256) -> Self {
        RecoveryMessage::Hash(hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::PrivateKey;

    #[test]
    fn recover_signature_from_message() {
        let message = "Some data";
        let hash = hash_message(message);
        let key = PrivateKey::new(&mut rand::thread_rng());
        let address = Address::from(&key);

        // sign a message
        let signature = key.sign(message);

        // ecrecover via the message will hash internally
        let recovered = signature.recover(message).unwrap();

        // if provided with a hash, it will skip hashing
        let recovered2 = signature.recover(hash).unwrap();

        // verifies the signature is produced by `address`
        signature.verify(message, address).unwrap();

        assert_eq!(recovered, address);
        assert_eq!(recovered2, address);
    }

    #[test]
    fn recover_web3_signature() {
        // test vector taken from:
        // https://web3js.readthedocs.io/en/v1.2.2/web3-eth-accounts.html#sign
        let key: PrivateKey = "4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318"
            .parse()
            .unwrap();
        let address = Address::from(&key);
        let signature = Signature::from_str(
            "b91467e570a6466aa9e9876cbcd013baba02900b8979d43fe208a4a4f339f5fd6007e74cd82e037b800186422fc2da167c747ef045e5d18a5f5d4300f8e1a0291c"
        ).expect("could not parse signature");

        // recover the address from the above signature
        let recovered = signature.recover("Some data").unwrap();
        assert_eq!(
            format!("{:?}", recovered),
            "0x2c7536e3605d9c16a7a3d7b1898e529396a65c23"
        );

        assert_eq!(recovered, address);
    }

    #[test]
    fn to_vec() {
        let message = "Some data";
        let key = PrivateKey::new(&mut rand::thread_rng());
        let signature = key.sign(message);
        let serialized = signature.to_vec();
        let de = Signature::try_from(&serialized[..]).unwrap();
        assert_eq!(signature, de);
    }
}
