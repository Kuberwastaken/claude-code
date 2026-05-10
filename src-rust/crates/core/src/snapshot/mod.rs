pub mod types;
pub mod shadow;
pub mod registry;

pub use types::{FileDiff, FileStatus, Patch};
pub use shadow::ShadowSnapshot;
pub use registry::{get_or_create, remove};
