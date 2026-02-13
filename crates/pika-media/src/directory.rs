#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryMessage {
    pub version: u8,
    pub entries: Vec<String>,
}
