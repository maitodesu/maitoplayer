//! Deterministic clip/frame policy and content-addressed mining assets.

pub mod preview;
pub mod spec;
pub mod store;
pub mod workflow;

pub use spec::{AssetSpec, ClipPolicy};
pub use store::{AssetKind, AssetStore, CleanupReport, StoredAsset};
pub use workflow::{AssetBundle, AssetRetentionPolicy, MiningAssetService};
