use std::path::PathBuf;

use color_eyre::{eyre::Result, Report};

pub struct Args {
    pub config_location: ConfigurationLocation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigurationLocation {
    S3 { bucket: String, key: String },
    Filesystem(PathBuf),
}

impl ConfigurationLocation {
    pub async fn fetch(&self) -> Result<Vec<u8>> {
        let bytes = match self {
            Self::Filesystem(path) => tokio::fs::read(path).await?,
            Self::S3 { bucket, key } => {
                let config = aws_config::load_from_env().await;
                let client = aws_sdk_s3::Client::new(&config);

                let response = client.get_object().bucket(bucket).key(key).send().await?;
                let bytes = response.body.collect().await?;

                bytes.to_vec()
            }
        };

        Ok(bytes)
    }
}

impl Args {
    pub fn parse() -> Result<Self> {
        let args = pico_args::Arguments::from_env();

        Self::try_from(args)
    }
}

impl TryFrom<pico_args::Arguments> for Args {
    type Error = Report;

    fn try_from(mut args: pico_args::Arguments) -> Result<Self> {
        let config: String = args.value_from_str("--config")?;

        let config_location = config
            .strip_prefix("s3://")
            .map(|bucket_and_key| {
                let (bucket, key) = bucket_and_key.split_once('/').expect("Invalid S3 URI");

                ConfigurationLocation::S3 {
                    bucket: String::from(bucket),
                    key: String::from(key),
                }
            })
            .unwrap_or_else(|| ConfigurationLocation::Filesystem(config.into()));

        Ok(Self { config_location })
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use color_eyre::Result;

    use crate::args::{Args, ConfigurationLocation};

    #[test]
    fn can_determine_filesystem_config() -> Result<()> {
        let raw_args = vec![OsString::from("--config"), OsString::from("f2.yaml")];

        let args = pico_args::Arguments::from_vec(raw_args);
        let parsed = Args::try_from(args)?;

        let expected = ConfigurationLocation::Filesystem(PathBuf::from("f2.yaml"));

        assert_eq!(parsed.config_location, expected);

        Ok(())
    }

    #[test]
    fn can_determine_s3_config() -> Result<()> {
        let raw_args = vec![
            OsString::from("--config"),
            OsString::from("s3://some-bucket/some-key.yaml"),
        ];

        let args = pico_args::Arguments::from_vec(raw_args);
        let parsed = Args::try_from(args)?;

        let expected = ConfigurationLocation::S3 {
            bucket: String::from("some-bucket"),
            key: String::from("some-key.yaml"),
        };

        assert_eq!(parsed.config_location, expected);

        Ok(())
    }

    #[test]
    #[should_panic]
    fn invalid_s3_uri_will_panic() {
        let raw_args = vec![
            OsString::from("--config"),
            // missing a key here
            OsString::from("s3://some-bucket"),
        ];

        let args = pico_args::Arguments::from_vec(raw_args);
        let _ = Args::try_from(args);
    }
}
