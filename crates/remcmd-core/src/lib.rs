#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionProfile {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
}

impl ConnectionProfile {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        host: impl Into<String>,
        port: u16,
        username: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            host: host.into(),
            port: port,
            username: username.into(),
        }
    }

    pub fn samples() -> Vec<Self> {
        vec![
            Self::new("local-dev", "Local Dev", "127.0.0.1", 22, "dev"),
            Self::new("staging", "Staging", "192.168.1.10", 22, "ubuntu"),
        ]
    }

    pub fn address(&self) -> String {
        format!("{}@{}:{}", self.username, self.host, self.port)
    }
}
