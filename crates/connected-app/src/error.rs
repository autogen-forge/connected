use connected_core::LayoutError;
use connected_core::PlatformError;
use connected_net::NetError;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Net(#[from] NetError),
    #[error(transparent)]
    Platform(#[from] PlatformError),
    #[error(transparent)]
    Layout(#[from] LayoutError),
    #[error("{0}")]
    Config(String),
}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        AppError::Net(NetError::from(err))
    }
}

pub type Result<T> = std::result::Result<T, AppError>;
