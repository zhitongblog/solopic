use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use image::{DynamicImage, ImageDecoder, ImageReader};
use rayon::prelude::*;

use crate::locale::tr;
use crate::report::Report;

pub const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "bmp", "webp", "gif", "tif", "tiff"];

#[derive(Debug, thiserror::Error)]
pub enum PicError {
    #[error("{0}")]
    Msg(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Image(#[from] image::ImageError),
}

pub(crate) fn dir_not_found(dir: &Path) -> PicError {
    PicError::Msg(format!("{}: {}", tr("目录不存在", "Directory not found"), dir.display()))
}

pub fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

pub fn list_images(dir: &Path) -> Result<Vec<PathBuf>, PicError> {
    if !dir.is_dir() {
        return Err(dir_not_found(dir));
    }
    let mut files: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_image(p))
        .collect();
    files.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    Ok(files)
}

#[derive(Debug, Clone)]
pub enum OutputMode {
    InPlace,
    Dir(PathBuf),
}

impl OutputMode {
    pub fn dest_for(&self, src: &Path) -> PathBuf {
        match self {
            OutputMode::InPlace => src.to_path_buf(),
            OutputMode::Dir(d) => d.join(src.file_name().unwrap_or_default()),
        }
    }

    pub fn prepare(&self) -> Result<(), PicError> {
        if let OutputMode::Dir(d) = self {
            fs::create_dir_all(d)?;
        }
        Ok(())
    }
}

/// 读图并自动应用 EXIF 方向（手机竖拍照片按肉眼所见方向处理）。
pub fn load_oriented(path: &Path) -> Result<DynamicImage, PicError> {
    let mut decoder = ImageReader::open(path)?.with_guessed_format()?.into_decoder()?;
    let orientation = decoder.orientation().ok();
    let mut img = DynamicImage::from_decoder(decoder)?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    Ok(img)
}

fn is_animated_gif(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    if ext != "gif" {
        return false;
    }
    use image::codecs::gif::GifDecoder;
    use image::AnimationDecoder;
    let Ok(f) = fs::File::open(path) else { return false };
    let Ok(dec) = GifDecoder::new(std::io::BufReader::new(f)) else { return false };
    dec.into_frames().take(2).count() > 1
}

pub fn temp_sibling(dst: &Path, tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let name = dst.file_name().unwrap_or_default().to_string_lossy();
    dst.with_file_name(format!(".pic-{tag}-{nanos:x}-{name}"))
}

/// 保存：先写同目录临时文件再原子替换，避免中断留下半截文件。
/// JPEG 质量 95；含 alpha 的图存 JPEG 时自动转 RGB。
pub fn save_image(img: &DynamicImage, dst: &Path) -> Result<(), PicError> {
    let ext = dst
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    let tmp = temp_sibling(dst, "tmp");
    let result = (|| -> Result<(), PicError> {
        match ext.as_str() {
            "jpg" | "jpeg" => {
                let file = fs::File::create(&tmp)?;
                let mut w = std::io::BufWriter::new(file);
                let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut w, 95);
                match img {
                    DynamicImage::ImageRgb8(_) | DynamicImage::ImageLuma8(_) => {
                        img.write_with_encoder(enc)?
                    }
                    _ => DynamicImage::ImageRgb8(img.to_rgb8()).write_with_encoder(enc)?,
                }
            }
            _ => img.save_with_format(
                &tmp,
                image::ImageFormat::from_path(dst).unwrap_or(image::ImageFormat::Png),
            )?,
        }
        fs::rename(&tmp, dst).or_else(|_| {
            fs::remove_file(dst).ok();
            fs::rename(&tmp, dst)
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// 单个文件的处理结果：完成（结果图 + 附加说明）或跳过（原因）。
pub enum OpResult {
    Done(DynamicImage, Option<String>),
    Skip(String),
}

/// 批处理骨架：并行加载→处理→保存，单文件失败不影响整批。
pub fn run_batch<F>(files: &[PathBuf], out: &OutputMode, op: F) -> Report
where
    F: Fn(&DynamicImage) -> Result<OpResult, String> + Sync,
{
    #[derive(Debug)]
    enum Outcome {
        Ok { output: String, detail: Option<String> },
        Skip(String),
        Err(String),
    }

    let mut report = Report::default();
    if let Err(e) = out.prepare() {
        report.push_err(tr("<输出目录>", "<output dir>"), e.to_string());
        return report;
    }

    let results: Vec<(String, Outcome)> = files
        .par_iter()
        .map(|f| {
            let name = f.to_string_lossy().to_string();
            if is_animated_gif(f) {
                return (
                    name,
                    Outcome::Skip(tr("多帧动图不支持，已跳过", "Animated GIF not supported, skipped")),
                );
            }
            let img = match load_oriented(f) {
                Ok(i) => i,
                Err(e) => {
                    return (name, Outcome::Err(format!("{}: {e}", tr("读取失败", "Read failed"))))
                }
            };
            match op(&img) {
                Ok(OpResult::Done(result, detail)) => {
                    let dst = out.dest_for(f);
                    match save_image(&result, &dst) {
                        Ok(()) => (
                            name,
                            Outcome::Ok { output: dst.to_string_lossy().to_string(), detail },
                        ),
                        Err(e) => {
                            (name, Outcome::Err(format!("{}: {e}", tr("保存失败", "Save failed"))))
                        }
                    }
                }
                Ok(OpResult::Skip(r)) => (name, Outcome::Skip(r)),
                Err(e) => (name, Outcome::Err(e)),
            }
        })
        .collect();

    for (file, outcome) in results {
        match outcome {
            Outcome::Ok { output, detail } => report.push_ok(file, Some(output), detail),
            Outcome::Skip(r) => report.push_skip(file, r),
            Outcome::Err(e) => report.push_err(file, e),
        }
    }
    report
}
