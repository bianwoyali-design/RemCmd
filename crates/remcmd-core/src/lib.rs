#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionProfile {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
}

impl ConnectionProfile {
    pub fn sample() -> Self {
        Self {
            id: "local-dev".into(),
            name: "Local Dev".into(),
            host: "127.0.0.1".into(),
            port: 22,
            username: "dev".into(),
        }
    }
}
