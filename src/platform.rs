use crate::model::{Platform, RecordingTask, ResolvedStream};
use async_trait::async_trait;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ResolveError {
    #[error("the room is offline")]
    Offline,
    #[error("authentication is required before this room can be resolved")]
    AuthenticationRequired,
    #[error("{0} stream discovery is not configured")]
    NotConfigured(Platform),
    #[error("stream discovery failed: {0}")]
    Failed(String),
}

/// Finds a direct media URL for a room using an account/session that the user is
/// authorized to use. Implementations must not bypass DRM or access controls.
#[async_trait]
pub trait StreamResolver: Send + Sync {
    fn platform(&self) -> Platform;
    async fn resolve(&self, task: &RecordingTask) -> Result<ResolvedStream, ResolveError>;
}

/// Explicit placeholder for a platform integration. The application can launch
/// with this resolver, while UI/account integration is developed separately.
pub struct UnconfiguredResolver {
    platform: Platform,
}

impl UnconfiguredResolver {
    pub fn new(platform: Platform) -> Self {
        Self { platform }
    }
}

#[async_trait]
impl StreamResolver for UnconfiguredResolver {
    fn platform(&self) -> Platform {
        self.platform
    }

    async fn resolve(&self, _task: &RecordingTask) -> Result<ResolvedStream, ResolveError> {
        Err(ResolveError::NotConfigured(self.platform))
    }
}
