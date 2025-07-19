use std::path::PathBuf;

use color_eyre::eyre::{eyre, Result};
use color_eyre::Report;

use crate::config::ExternalBytes;

pub struct Args {
    pub config_location: ExternalBytes,
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

        let config_location = match config.strip_prefix("s3://") {
            Some(bucket_and_key) => {
                let (bucket, key) = bucket_and_key
                    .split_once('/')
                    .ok_or_else(|| eyre!("invalid s3 bucket and key provided: {bucket_and_key}"))?;

                ExternalBytes::S3 {
                    bucket: bucket.to_owned(),
                    key: key.to_owned(),
                }
            }
            None => ExternalBytes::Filesystem {
                path: PathBuf::from(config),
            },
        };

        Ok(Self { config_location })
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use color_eyre::Result;

    use crate::args::Args;
    use crate::config::ExternalBytes;

    #[test]
    fn can_determine_filesystem_config() -> Result<()> {
        let raw_args = vec![OsString::from("--config"), OsString::from("f2.yaml")];

        let args = pico_args::Arguments::from_vec(raw_args);
        let parsed = Args::try_from(args)?;

        let expected = ExternalBytes::Filesystem {
            path: PathBuf::from("f2.yaml"),
        };

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

        let expected = ExternalBytes::S3 {
            bucket: String::from("some-bucket"),
            key: String::from("some-key.yaml"),
        };

        assert_eq!(parsed.config_location, expected);

        Ok(())
    }

    #[test]
    fn s3_uri_without_bucket_or_key_will_fail_to_parse() {
        let raw_args = vec![
            OsString::from("--config"),
            // missing a bucket and key
            OsString::from("s3://"),
        ];

        let args = pico_args::Arguments::from_vec(raw_args);

        let Err(e) = Args::try_from(args) else {
            panic!("successfully parsed bucket");
        };

        assert_eq!(e.to_string(), "invalid s3 bucket and key provided: ");
    }

    #[test]
    fn s3_uri_without_key_will_fail_to_parse() {
        let raw_args = vec![
            OsString::from("--config"),
            // missing a key here
            OsString::from("s3://some-bucket"),
        ];

        let args = pico_args::Arguments::from_vec(raw_args);

        let Err(e) = Args::try_from(args) else {
            panic!("successfully parsed bucket");
        };

        assert_eq!(
            e.to_string(),
            "invalid s3 bucket and key provided: some-bucket"
        );
    }
}
