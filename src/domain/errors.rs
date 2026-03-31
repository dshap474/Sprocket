use thiserror::Error;

#[derive(Debug, Error)]
pub enum SprocketError {
    #[error("manager state is missing anchor manifest `{0}`")]
    MissingAnchorManifest(String),
}
