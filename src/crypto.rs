use std::env;

use base64::alphabet::STANDARD;
use base64::engine::{GeneralPurpose, GeneralPurposeConfig};
use base64::Engine;
use color_eyre::Result;
use rsa::pkcs8::DecodePrivateKey;
use rsa::{Pkcs1v15Encrypt, RsaPrivateKey};

pub fn get_private_key(environment_variable: &str) -> Result<RsaPrivateKey> {
    let base64 = env::var(environment_variable)?;
    let decoded = base64_decode(&base64)?;
    let utf8 = String::from_utf8(decoded)?;
    let parsed = RsaPrivateKey::from_pkcs8_pem(&utf8)?;

    parsed.validate()?;

    Ok(parsed)
}

pub fn decrypt(secret: &str, key: &RsaPrivateKey) -> Result<String> {
    let decoded = base64_decode(secret)?;
    let decrypted = key.decrypt(Pkcs1v15Encrypt, &decoded)?;
    let decoded = String::from_utf8(decrypted)?;

    Ok(decoded)
}

fn base64_decode(value: &str) -> Result<Vec<u8>> {
    let engine = GeneralPurpose::new(&STANDARD, GeneralPurposeConfig::new());
    let decoded = engine.decode(value)?;

    Ok(decoded)
}
