#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io::Cursor;
use std::path::{Path, PathBuf};

use base64::Engine;
use serde::Serialize;
use tauri_plugin_dialog::DialogExt;

use pic_core::{
    adjust_files, adjust_image, crop_files, crop_image, enhance_files, enhance_image, list_images,
    load_oriented, AdjustSpec, CropSpec, EnhanceMode, EnhanceSpec, OutputMode, Report,
};

#[derive(Serialize)]
struct FileInfo {
    name: String,
    width: u32,
    height: u32,
    bytes: u64,
}

fn err_str<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

fn parse_mode(mode: &str) -> Result<EnhanceMode, String> {
    match mode {
        "color" => Ok(EnhanceMode::Color),
        "gray" => Ok(EnhanceMode::Gray),
        "bw" => Ok(EnhanceMode::Bw),
        other => Err(format!("未知模式 {other}")),
    }
}

fn resolve_out(dir: &str, output_dir: Option<String>, overwrite: bool) -> OutputMode {
    if overwrite {
        OutputMode::InPlace
    } else {
        OutputMode::Dir(
            output_dir.map(PathBuf::from).unwrap_or_else(|| Path::new(dir).join("pic-output")),
        )
    }
}

fn selected_files(dir: &str, files: &[String]) -> Result<Vec<PathBuf>, String> {
    if files.is_empty() {
        return Err("没有选中任何图片".into());
    }
    Ok(files.iter().map(|f| Path::new(dir).join(f)).collect())
}

fn to_data_url(img: &image::DynamicImage) -> Result<String, String> {
    let mut buf = Cursor::new(Vec::new());
    let rgb = image::DynamicImage::ImageRgb8(img.to_rgb8());
    rgb.write_to(&mut buf, image::ImageFormat::Jpeg).map_err(err_str)?;
    Ok(format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(buf.into_inner())
    ))
}

fn load_scaled(dir: &str, name: &str, max: u32) -> Result<image::DynamicImage, String> {
    let img = load_oriented(&Path::new(dir).join(name)).map_err(err_str)?;
    if img.width().max(img.height()) > max {
        Ok(img.resize(max, max, image::imageops::FilterType::Triangle))
    } else {
        Ok(img)
    }
}

/// 前端语言切换时同步核心引擎消息语言（zh → 中文，其余 → 英文）。
#[tauri::command]
fn set_locale(lang: String) {
    pic_core::set_locale_by_tag(&lang);
}

/// 内部自测钩子：PIC_AUTOTEST=1 时前端自动跑一遍"切页签→执行增强"流程。
#[tauri::command]
fn autotest_mode() -> bool {
    std::env::var("PIC_AUTOTEST").map(|v| v == "1").unwrap_or(false)
}

/// PIC_LANG 环境变量可强制界面语言（如截图/演示场景），优先于记住的选择。
#[tauri::command]
fn initial_lang() -> Option<String> {
    std::env::var("PIC_LANG").ok().filter(|v| !v.is_empty())
}

/// 支持 `pic-app <文件夹>` 直接打开（也支持把文件夹拖到 exe 上）。
#[tauri::command]
fn initial_dir() -> Option<String> {
    let arg = std::env::args().nth(1)?;
    let path = std::fs::canonicalize(&arg).ok()?;
    if path.is_dir() {
        let s = path.to_string_lossy().to_string();
        Some(s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s))
    } else {
        None
    }
}

#[tauri::command]
fn pick_folder(app: tauri::AppHandle) -> Option<String> {
    app.dialog()
        .file()
        .blocking_pick_folder()
        .and_then(|p| p.into_path().ok())
        .map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
fn pick_map_file(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let picked = app
        .dialog()
        .file()
        .add_filter("映射文件", &["txt", "csv"])
        .blocking_pick_file()
        .and_then(|p| p.into_path().ok());
    match picked {
        None => Ok(None),
        Some(path) => {
            let (pairs, errors) = pic_core::read_mapping_file(&path).map_err(err_str)?;
            let mut text = String::new();
            for (a, b) in &pairs {
                text.push_str(&format!("{a},{b}\n"));
            }
            for e in &errors {
                text.push_str(&format!("# 解析失败: {e}\n"));
            }
            Ok(Some(text))
        }
    }
}

#[tauri::command]
fn list_dir(dir: String) -> Result<Vec<FileInfo>, String> {
    let files = list_images(Path::new(&dir)).map_err(err_str)?;
    Ok(files
        .iter()
        .map(|f| {
            let (width, height) = image::image_dimensions(f).unwrap_or((0, 0));
            let bytes = f.metadata().map(|m| m.len()).unwrap_or(0);
            FileInfo {
                name: f.file_name().unwrap_or_default().to_string_lossy().to_string(),
                width,
                height,
                bytes,
            }
        })
        .collect())
}

#[tauri::command]
fn thumb(dir: String, name: String, max: u32) -> Result<String, String> {
    to_data_url(&load_scaled(&dir, &name, max.clamp(64, 1600))?)
}

/// 前后对比预览：在缩小图上执行真实管线，返回 (原图, 处理后) 两个 data URL。
#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn preview(
    dir: String,
    name: String,
    kind: String,
    crop: Option<CropSpec>,
    adjust: Option<AdjustSpec>,
    mode: Option<String>,
    deskew: Option<bool>,
    max_deskew_deg: Option<f32>,
    denoise: Option<bool>,
) -> Result<(String, String), String> {
    let src = load_scaled(&dir, &name, 900)?;
    let before = to_data_url(&src)?;
    let after = match kind.as_str() {
        "adjust" => {
            let spec = adjust.ok_or("缺少 adjust 参数")?;
            adjust_image(&src, &spec)
        }
        "enhance" => {
            let spec = EnhanceSpec {
                mode: parse_mode(mode.as_deref().unwrap_or("color"))?,
                deskew: deskew.unwrap_or(true),
                max_deskew_deg: max_deskew_deg.unwrap_or(20.0),
                denoise: denoise.unwrap_or(true),
                only_documents: false,
            };
            enhance_image(&src, &spec).image
        }
        "crop" => {
            // 预览图是缩小过的，按比例换算裁剪量
            let full = load_oriented(&Path::new(&dir).join(&name)).map_err(err_str)?;
            let spec = crop.ok_or("缺少 crop 参数")?;
            let cropped = crop_image(&full, &spec)?;
            cropped.resize(900, 900, image::imageops::FilterType::Triangle)
        }
        other => return Err(format!("未知预览类型 {other}")),
    };
    Ok((before, to_data_url(&after)?))
}

#[tauri::command]
fn run_crop(
    dir: String,
    files: Vec<String>,
    spec: CropSpec,
    output_dir: Option<String>,
    overwrite: bool,
) -> Result<Report, String> {
    spec.validate()?;
    let files = selected_files(&dir, &files)?;
    Ok(crop_files(&files, &spec, &resolve_out(&dir, output_dir, overwrite)))
}

#[tauri::command]
fn run_adjust(
    dir: String,
    files: Vec<String>,
    spec: AdjustSpec,
    output_dir: Option<String>,
    overwrite: bool,
) -> Result<Report, String> {
    spec.validate()?;
    let files = selected_files(&dir, &files)?;
    Ok(adjust_files(&files, &spec, &resolve_out(&dir, output_dir, overwrite)))
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn run_enhance(
    dir: String,
    files: Vec<String>,
    mode: String,
    deskew: bool,
    max_deskew_deg: f32,
    denoise: bool,
    only_documents: bool,
    output_dir: Option<String>,
    overwrite: bool,
) -> Result<Report, String> {
    let spec =
        EnhanceSpec { mode: parse_mode(&mode)?, deskew, max_deskew_deg, denoise, only_documents };
    let files = selected_files(&dir, &files)?;
    Ok(enhance_files(&files, &spec, &resolve_out(&dir, output_dir, overwrite)))
}

#[derive(Serialize)]
struct RenameResult {
    report: Report,
    parse_errors: Vec<String>,
    undo_log: Option<String>,
}

#[tauri::command]
fn run_rename(dir: String, mapping_text: String, execute: bool) -> Result<RenameResult, String> {
    let (mapping, parse_errors) = pic_core::parse_mapping(&mapping_text);
    let folder = Path::new(&dir);
    let (report, applied) = pic_core::batch_rename(folder, &mapping, execute).map_err(err_str)?;
    let undo_log = if execute && !applied.is_empty() {
        pic_core::write_undo_log(folder, &applied)
            .ok()
            .map(|p| p.to_string_lossy().to_string())
    } else {
        None
    };
    Ok(RenameResult { report, parse_errors, undo_log })
}

#[tauri::command]
fn run_undo(dir: String, undo_log: String) -> Result<Report, String> {
    let log = pic_core::read_undo_log(Path::new(&undo_log)).map_err(err_str)?;
    let (report, _) =
        pic_core::batch_rename(Path::new(&dir), &pic_core::rename::invert(&log), true)
            .map_err(err_str)?;
    Ok(report)
}

#[tauri::command]
fn open_in_explorer(path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer").arg(&path).spawn().map_err(err_str)?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(&path).spawn().map_err(err_str)?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(&path).spawn().map_err(err_str)?;
    }
    Ok(())
}

fn main() {
    pic_core::init_locale_from_env();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            set_locale,
            autotest_mode,
            initial_lang,
            initial_dir,
            pick_folder,
            pick_map_file,
            list_dir,
            thumb,
            preview,
            run_crop,
            run_adjust,
            run_enhance,
            run_rename,
            run_undo,
            open_in_explorer
        ])
        .run(tauri::generate_context!())
        .expect("pic 启动失败");
}
