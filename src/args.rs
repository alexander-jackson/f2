use anyhow::Result;

pub struct Args {
    config: Option<String>,
}

impl Args {
    pub fn parse() -> Result<Self> {
        let mut args = pico_args::Arguments::from_env();

        Ok(Self {
            config: args.opt_value_from_str("--config")?,
        })
    }

    pub fn get_config_path(&self) -> String {
        self.config
            .clone()
            .unwrap_or_else(|| String::from("f2.toml"))
    }
}
