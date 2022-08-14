#[derive(Clone, Debug)]
pub struct Container {
    pub image: String,
    pub target_port: u16,
}

#[derive(Clone, Debug)]
pub struct Registry {
    pub base: Option<String>,
    pub repository: String,
    pub username: Option<String>,
    pub password: Option<String>,
}
