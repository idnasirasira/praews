use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("template error: {0}")]
    Template(#[from] tera::Error),

    #[error("configuration error: {0}")]
    Config(String),

    #[error("process error: {0}")]
    Process(String),

    #[error("port {0} is already in use")]
    PortInUse(u16),

    #[error("{0}")]
    Other(String),
}

impl From<anyhow::Error> for CoreError {
    fn from(value: anyhow::Error) -> Self {
        CoreError::Other(value.to_string())
    }
}
