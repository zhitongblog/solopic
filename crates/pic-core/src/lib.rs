//! pic-core：批量图像处理核心库。
//! 一核多壳架构中的"核"——被 CLI / MCP Server / Tauri GUI 三个薄壳复用。

pub mod adjust;
pub mod crop;
pub mod enhance;
pub mod io;
pub mod locale;
pub mod rename;
pub mod report;

pub use adjust::{adjust_files, adjust_image, AdjustSpec};
pub use crop::{crop_files, crop_image, CropSpec};
pub use enhance::{enhance_files, enhance_image, looks_like_document, EnhanceMode, EnhanceSpec};
pub use io::{list_images, load_oriented, save_image, OutputMode, PicError, IMAGE_EXTS};
pub use locale::{init_locale_from_env, set_locale, set_locale_by_tag, tr, Locale};
pub use rename::{batch_rename, parse_mapping, read_mapping_file, read_undo_log, write_undo_log};
pub use report::{Entry, Report};
