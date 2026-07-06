use std::path::{Path, PathBuf};

use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::stdio,
    ErrorData as McpError, ServerHandler, ServiceExt,
};
use schemars::JsonSchema;
use serde::Deserialize;

use pic_core::{
    adjust_files, crop_files, enhance_files, list_images, AdjustSpec, CropSpec, EnhanceMode,
    EnhanceSpec, OutputMode, Report,
};

fn resolve_output(folder: &Path, output_dir: Option<String>, overwrite: bool) -> OutputMode {
    if overwrite {
        OutputMode::InPlace
    } else {
        OutputMode::Dir(output_dir.map(PathBuf::from).unwrap_or_else(|| folder.join("pic-output")))
    }
}

fn report_result(report: &Report) -> Result<CallToolResult, McpError> {
    let json = serde_json::to_string_pretty(report)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::success(vec![ContentBlock::text(json)]))
}

fn files_of(folder: &str) -> Result<Vec<PathBuf>, McpError> {
    list_images(Path::new(folder)).map_err(|e| McpError::invalid_params(e.to_string(), None))
}

#[derive(Deserialize, JsonSchema)]
struct FolderArgs {
    /// Absolute path of the folder containing images
    folder: String,
}

#[derive(Deserialize, JsonSchema)]
struct CropArgs {
    /// Absolute path of the folder containing images
    folder: String,
    /// Pixels to cut from the left edge
    #[serde(default)]
    left: u32,
    /// Pixels to cut from the top edge
    #[serde(default)]
    top: u32,
    /// Pixels to cut from the right edge
    #[serde(default)]
    right: u32,
    /// Pixels to cut from the bottom edge
    #[serde(default)]
    bottom: u32,
    /// Output directory (default <folder>/pic-output)
    output_dir: Option<String>,
    /// Overwrite originals in place
    #[serde(default)]
    overwrite: bool,
}

#[derive(Deserialize, JsonSchema)]
struct AdjustArgs {
    /// Absolute path of the folder containing images
    folder: String,
    /// Brightness factor, 1.0 = unchanged, 1.2 = +20%
    #[serde(default = "one")]
    brightness: f32,
    /// Contrast factor, 1.0 = unchanged
    #[serde(default = "one")]
    contrast: f32,
    /// Saturation factor, 1.0 = unchanged
    #[serde(default = "one")]
    saturation: f32,
    /// Sharpness factor, 1.0 = unchanged
    #[serde(default = "one")]
    sharpness: f32,
    /// Convert to grayscale
    #[serde(default)]
    grayscale: bool,
    /// Output directory (default <folder>/pic-output)
    output_dir: Option<String>,
    /// Overwrite originals in place
    #[serde(default)]
    overwrite: bool,
}

fn one() -> f32 {
    1.0
}

#[derive(Deserialize, JsonSchema)]
struct EnhanceArgs {
    /// Absolute path of the folder containing images
    folder: String,
    /// Output mode: color (default) / gray / bw (pure black & white document)
    mode: Option<String>,
    /// Auto-deskew (default true)
    deskew: Option<bool>,
    /// Max deskew search angle in degrees, 1-45 (default 20)
    max_deskew_deg: Option<f32>,
    /// Denoise (default true)
    denoise: Option<bool>,
    /// Only process images detected as documents, skip photos (default false)
    #[serde(default)]
    only_documents: bool,
    /// Output directory (default <folder>/pic-output)
    output_dir: Option<String>,
    /// Overwrite originals in place
    #[serde(default)]
    overwrite: bool,
}

#[derive(Deserialize, JsonSchema)]
struct RenameArgs {
    /// Absolute path of the folder containing the files
    folder: String,
    /// Path to a mapping file, one "old,new" per line (alternative to mapping_text)
    map_file: Option<String>,
    /// Mapping text, one "old,new" per line (alternative to map_file)
    mapping_text: Option<String>,
    /// false = dry-run preview only (default); true = apply renames and write an undo log
    #[serde(default)]
    execute: bool,
}

#[derive(Clone)]
struct PicServer {
    // #[tool_handler] 宏在 trait 实现里引用该字段，dead_code 分析看不到
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl PicServer {
    fn new() -> Self {
        Self { tool_router: Self::tool_router() }
    }

    #[tool(description = "List all image files in a folder")]
    fn list_images(&self, Parameters(args): Parameters<FolderArgs>) -> Result<CallToolResult, McpError> {
        let files = files_of(&args.folder)?;
        let names: Vec<String> = files
            .iter()
            .map(|f| f.file_name().unwrap_or_default().to_string_lossy().to_string())
            .collect();
        Ok(CallToolResult::success(vec![ContentBlock::text(
            serde_json::json!({ "count": names.len(), "files": names }).to_string(),
        )]))
    }

    #[tool(description = "Batch crop: cut N pixels off the left/top/right/bottom edges of every image in a folder. Per-image relative cropping, so image sizes may differ (e.g. left=100, bottom=57).")]
    fn batch_crop(&self, Parameters(args): Parameters<CropArgs>) -> Result<CallToolResult, McpError> {
        let files = files_of(&args.folder)?;
        let spec = CropSpec { left: args.left, top: args.top, right: args.right, bottom: args.bottom };
        spec.validate().map_err(|e| McpError::invalid_params(e, None))?;
        let out = resolve_output(Path::new(&args.folder), args.output_dir, args.overwrite);
        report_result(&crop_files(&files, &spec, &out))
    }

    #[tool(description = "Batch adjust brightness / contrast / saturation / sharpness (factor 1.0 = unchanged, 1.2 = +20%), optionally convert to grayscale.")]
    fn batch_adjust(&self, Parameters(args): Parameters<AdjustArgs>) -> Result<CallToolResult, McpError> {
        let files = files_of(&args.folder)?;
        let spec = AdjustSpec {
            brightness: args.brightness,
            contrast: args.contrast,
            saturation: args.saturation,
            sharpness: args.sharpness,
            grayscale: args.grayscale,
        };
        spec.validate().map_err(|e| McpError::invalid_params(e, None))?;
        let out = resolve_output(Path::new(&args.folder), args.output_dir, args.overwrite);
        report_result(&adjust_files(&files, &spec, &out))
    }

    #[tool(description = "Smart-enhance scanned or photographed documents: remove shadows, even out lighting, whiten background, darken text, auto-deskew. mode=color for enhanced color, gray for grayscale, bw for pure black-and-white scan look.")]
    fn batch_enhance(&self, Parameters(args): Parameters<EnhanceArgs>) -> Result<CallToolResult, McpError> {
        let files = files_of(&args.folder)?;
        let mode = match args.mode.as_deref().unwrap_or("color") {
            "color" => EnhanceMode::Color,
            "gray" => EnhanceMode::Gray,
            "bw" => EnhanceMode::Bw,
            other => return Err(McpError::invalid_params(format!("unknown mode {other}"), None)),
        };
        let spec = EnhanceSpec {
            mode,
            deskew: args.deskew.unwrap_or(true),
            max_deskew_deg: args.max_deskew_deg.unwrap_or(20.0),
            denoise: args.denoise.unwrap_or(true),
            only_documents: args.only_documents,
        };
        let out = resolve_output(Path::new(&args.folder), args.output_dir, args.overwrite);
        report_result(&enhance_files(&files, &spec, &out))
    }

    #[tool(description = "Batch rename files from a mapping, one \"old,new\" per line (full-width comma and tab also accepted; if the new name has no extension the original one is kept). execute=false previews and validates only (default); execute=true applies and writes an undo log.")]
    fn batch_rename(&self, Parameters(args): Parameters<RenameArgs>) -> Result<CallToolResult, McpError> {
        let (mapping, parse_errors) = if let Some(file) = &args.map_file {
            pic_core::read_mapping_file(Path::new(file))
                .map_err(|e| McpError::invalid_params(e.to_string(), None))?
        } else if let Some(text) = &args.mapping_text {
            pic_core::parse_mapping(text)
        } else {
            return Err(McpError::invalid_params("map_file or mapping_text is required", None));
        };
        let folder = Path::new(&args.folder);
        let (report, applied) = pic_core::batch_rename(folder, &mapping, args.execute)
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;
        let undo_log = if args.execute && !applied.is_empty() {
            pic_core::write_undo_log(folder, &applied).ok().map(|p| p.to_string_lossy().to_string())
        } else {
            None
        };
        let json = serde_json::json!({
            "report": report,
            "parse_errors": parse_errors,
            "undo_log": undo_log,
        });
        Ok(CallToolResult::success(vec![ContentBlock::text(
            serde_json::to_string_pretty(&json).unwrap_or_default(),
        )]))
    }
}

#[tool_handler]
impl ServerHandler for PicServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "SoloPic batch image processing: batch_crop (edge crop) / batch_rename (mapping rename) / batch_adjust (brightness etc.) / batch_enhance (smart document cleanup) / list_images. Always use absolute paths.",
        )
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    pic_core::init_locale_from_env();
    let service = PicServer::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
