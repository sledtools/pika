#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConfig {
    pub moq_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaSession {
    config: SessionConfig,
    connected: bool,
}

impl MediaSession {
    pub fn new(config: SessionConfig) -> Self {
        Self {
            config,
            connected: false,
        }
    }

    pub fn connect(&mut self) {
        self.connected = true;
    }

    pub fn disconnect(&mut self) {
        self.connected = false;
    }

    pub fn is_connected(&self) -> bool {
        self.connected
    }

    pub fn config(&self) -> &SessionConfig {
        &self.config
    }
}
