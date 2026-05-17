//! User media asset library.

pub mod index;
pub mod library;
pub mod watcher;

pub use index::{
    AssetEvent, AssetIndex, AssetScanStatus, AssetWarning, INDEX_VERSION, MediaAssetRecord,
};
pub use library::{
    AssetLibrary, AssetLibraryError, AssetLibraryLimits, AssetTypeHint, AssetUploadOptions,
    AssetUpsert,
};
pub use watcher::{AssetWatchEvent, AssetWatcher};
