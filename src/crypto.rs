use base64::alphabet::STANDARD;
use base64::engine::{GeneralPurpose, GeneralPurposeConfig};
use base64::Engine;
use color_eyre::Result;
use rsa::pkcs8::DecodePrivateKey;
use rsa::{Pkcs1v15Encrypt, RsaPrivateKey};

pub fn parse_private_key(bytes: &[u8]) -> Result<RsaPrivateKey> {
    let utf8 = std::str::from_utf8(bytes)?;
    let parsed = RsaPrivateKey::from_pkcs8_pem(utf8)?;

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
