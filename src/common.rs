use std::{collections::HashMap, fmt};

use color_eyre::eyre::{eyre, Result, WrapErr};
use rsa::RsaPrivateKey;

use crate::config::{Service, VolumeDefinition};
use crate::crypto::decrypt;

#[derive(Clone)]
pub struct EncryptedEnvironment {
    variables: HashMap<String, String>,
}

impl EncryptedEnvironment {
    pub fn decrypt(&self, private_key: Option<&RsaPrivateKey>) -> Result<Environment> {
        let mut variables = HashMap::new();

        for (key, value) in self.variables.clone().into_iter() {
            tracing::info!("Resolving secret for {key}");

            let value = match value.strip_prefix("secret:") {
                Some(value) => {
                    let private_key = private_key
                        .ok_or_else(|| eyre!("Tried to decrypt secret without a key"))?;

                    decrypt(value, private_key)
                        .wrap_err_with(|| format!("Failed to decrypt secret value for '{key}'"))?
                }
                None => value,
            };

            variables.insert(key, value);
        }

        Ok(Environment { variables })
    }
}

#[derive(Clone)]
pub struct Environment {
    pub variables: HashMap<String, String>,
}

#[derive(Clone)]
pub struct Container {
    pub image: String,
    pub target_port: u16,
    pub environment: EncryptedEnvironment,
    pub volumes: HashMap<String, VolumeDefinition>,
}

impl Container {
    pub fn decrypt_environment(
        &self,
        private_key: Option<&RsaPrivateKey>,
    ) -> Result<Option<Environment>> {
        let decrypted = self.environment.decrypt(private_key)?;

        Ok(Some(decrypted))
    }
}

impl fmt::Debug for Container {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Container")
            .field("image", &self.image)
            .field("target_port", &self.target_port)
            .finish()
    }
}

impl From<&Service> for Container {
    fn from(service: &Service) -> Self {
        Self {
            image: service.image.clone(),
            target_port: service.port,
            environment: EncryptedEnvironment {
                variables: service.environment.clone(),
            },
            volumes: service.volumes.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use color_eyre::eyre::{eyre, Result};
    use rsa::{Pkcs1v15Encrypt, RsaPrivateKey, RsaPublicKey};

    use super::EncryptedEnvironment;

    fn generate_keys() -> Result<(RsaPublicKey, RsaPrivateKey)> {
        let mut rng = rand::thread_rng();
        let bits = 256;

        let private_key = RsaPrivateKey::new(&mut rng, bits)?;
        let public_key = RsaPublicKey::from(&private_key);

        Ok((public_key, private_key))
    }

    fn encrypt_and_encode_value(value: &str, public_key: &RsaPublicKey) -> Result<String> {
        use base64::{engine::general_purpose, Engine as _};

        let mut rng = rand::thread_rng();

        let encrypted = public_key.encrypt(&mut rng, Pkcs1v15Encrypt, value.as_bytes())?;

        Ok(general_purpose::STANDARD.encode(encrypted))
    }

    #[test]
    fn environments_can_be_decrypted() -> Result<()> {
        let (public, private) = generate_keys()?;

        let plaintext = "foobar";
        let encoded = encrypt_and_encode_value(plaintext, &public)?;

        let mut variables = HashMap::new();
        variables.insert(String::from("key"), format!("secret:{encoded}"));

        let encrypted_environment = EncryptedEnvironment { variables };

        let decrypted = encrypted_environment.decrypt(Some(&private))?;
        let value = decrypted
            .variables
            .get("key")
            .ok_or_else(|| eyre!("Failed to get a value for the key"))?;

        assert_eq!(value, plaintext);

        Ok(())
    }

    #[test]
    fn unencrypted_keys_are_left_alone() -> Result<()> {
        let mut variables = HashMap::new();
        variables.insert(String::from("key"), String::from("value"));

        let encrypted_environment = EncryptedEnvironment { variables };

        let decrypted = encrypted_environment.decrypt(None)?;
        let value = decrypted
            .variables
            .get("key")
            .ok_or_else(|| eyre!("Failed to get a value for the key"))?;

        assert_eq!(value, "value");

        Ok(())
    }

    #[test]
    fn decryption_errors_if_secrets_exist_without_private_key() -> Result<()> {
        let (public, _) = generate_keys()?;

        let plaintext = "foobar";
        let encoded = encrypt_and_encode_value(plaintext, &public)?;

        let mut variables = HashMap::new();
        variables.insert(String::from("key"), format!("secret:{encoded}"));

        let encrypted_environment = EncryptedEnvironment { variables };

        let decrypted = encrypted_environment.decrypt(None);

        assert!(decrypted.is_err());

        Ok(())
    }

    #[test]
    fn decryption_failures_return_an_error() -> Result<()> {
        let (public, _) = generate_keys()?;
        let (_, unrelated_private) = generate_keys()?;

        let plaintext = "foobar";
        let encoded = encrypt_and_encode_value(plaintext, &public)?;

        let mut variables = HashMap::new();
        variables.insert(String::from("key"), format!("secret:{encoded}"));

        let encrypted_environment = EncryptedEnvironment { variables };

        let decrypted = encrypted_environment.decrypt(Some(&unrelated_private));

        assert!(decrypted.is_err());

        Ok(())
    }
}
