use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use pic_core::{
    adjust_files, crop_files, enhance_files, list_images, tr, AdjustSpec, CropSpec, EnhanceMode,
    EnhanceSpec, OutputMode, Report,
};

#[derive(Parser)]
#[command(
    name = "pic",
    version,
    about = tr(
        "SoloPic — 免费批量图像处理：按边裁剪 / 映射改名 / 亮度对比度 / 智能文档增强",
        "SoloPic — free batch image processing: edge crop / mapping rename / brightness & contrast / smart document enhancement"
    ),
    subcommand_required = true,
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// JSON output for scripting / 以 JSON 输出结果
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Crop N pixels off each edge, per-image / 批量裁剪：从边缘切掉指定像素
    Crop {
        #[command(flatten)]
        io: IoArgs,
        /// Pixels to cut from the left / 左边切掉的像素
        #[arg(short = 'l', long, default_value_t = 0)]
        left: u32,
        /// Pixels to cut from the top / 上边切掉的像素
        #[arg(short = 't', long, default_value_t = 0)]
        top: u32,
        /// Pixels to cut from the right / 右边切掉的像素
        #[arg(short = 'r', long, default_value_t = 0)]
        right: u32,
        /// Pixels to cut from the bottom / 下边切掉的像素
        #[arg(short = 'b', long, default_value_t = 0)]
        bottom: u32,
    },
    /// Adjust brightness/contrast/saturation/sharpness (1.0 = unchanged) / 批量调整
    Adjust {
        #[command(flatten)]
        io: IoArgs,
        #[arg(long, default_value_t = 1.0)]
        brightness: f32,
        #[arg(long, default_value_t = 1.0)]
        contrast: f32,
        #[arg(long, default_value_t = 1.0)]
        saturation: f32,
        #[arg(long, default_value_t = 1.0)]
        sharpness: f32,
        /// Convert to grayscale / 转为灰度
        #[arg(long)]
        grayscale: bool,
    },
    /// Smart-enhance scanned/photographed documents / 智能增强扫描或拍照的文档
    Enhance {
        #[command(flatten)]
        io: IoArgs,
        /// color (default) / gray / bw
        #[arg(long, default_value = "color")]
        mode: String,
        /// Disable auto deskew / 关闭自动纠斜
        #[arg(long)]
        no_deskew: bool,
        /// Max deskew angle in degrees, 1-45 / 纠斜最大角度（度）
        #[arg(long, default_value_t = 20.0)]
        max_deskew: f32,
        /// Disable denoise / 关闭去噪
        #[arg(long)]
        no_denoise: bool,
        /// Only process images detected as documents / 仅处理判定为文档的图片
        #[arg(long)]
        only_documents: bool,
    },
    /// Rename files from a mapping file (dry-run by default, -x to apply) / 按映射文件批量改名
    Rename {
        /// Folder containing the files / 图片所在目录
        folder: PathBuf,
        /// Mapping file, one "old,new" per line / 映射文件：每行"旧名,新名"
        #[arg(short = 'm', long)]
        map: Option<PathBuf>,
        /// Apply the renames (default is dry-run preview) / 真正执行（默认只预览）
        #[arg(short = 'x', long)]
        execute: bool,
        /// Undo a previous rename via its undo log / 按 undo 日志撤销
        #[arg(long, conflicts_with = "map")]
        undo: Option<PathBuf>,
    },
    /// List images in a folder / 列出目录中的图片
    Ls { folder: PathBuf },
}

#[derive(Args)]
struct IoArgs {
    /// A folder, or one or more image files / 一个目录或若干图片文件
    #[arg(required = true)]
    paths: Vec<PathBuf>,
    /// Output directory (default <folder>/pic-output) / 输出目录
    #[arg(short = 'o', long, conflicts_with = "overwrite")]
    output: Option<PathBuf>,
    /// Overwrite originals in place / 覆盖原图
    #[arg(long)]
    overwrite: bool,
}

impl IoArgs {
    fn resolve(&self) -> Result<(Vec<PathBuf>, OutputMode)> {
        let (files, base) = if self.paths.len() == 1 && self.paths[0].is_dir() {
            let dir = &self.paths[0];
            (list_images(dir)?, dir.clone())
        } else {
            for p in &self.paths {
                if !p.is_file() {
                    bail!("{}: {}", tr("文件不存在", "File not found"), p.display());
                }
            }
            let base = self.paths[0]
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."));
            (self.paths.clone(), base)
        };
        if files.is_empty() {
            bail!(
                "{} ({})",
                tr("没有找到图片文件", "No image files found"),
                pic_core::IMAGE_EXTS.join("/")
            );
        }
        let out = if self.overwrite {
            OutputMode::InPlace
        } else {
            OutputMode::Dir(self.output.clone().unwrap_or_else(|| base.join("pic-output")))
        };
        Ok((files, out))
    }
}

fn print_report(report: &Report, json: bool) -> ExitCode {
    if json {
        println!("{}", serde_json::to_string_pretty(report).unwrap_or_default());
    } else {
        for e in &report.ok {
            let arrow = e.output.as_deref().unwrap_or("");
            let detail = e.detail.as_deref().map(|d| format!("  ({d})")).unwrap_or_default();
            println!("✓ {} -> {}{}", e.file, arrow, detail);
        }
        for e in &report.skipped {
            println!("- {}  [{}]", e.file, e.detail.as_deref().unwrap_or(""));
        }
        for e in &report.errors {
            println!("✗ {}  [{}]", e.file, e.detail.as_deref().unwrap_or(""));
        }
        if report.dry_run {
            println!(
                "\n{} {}{}",
                tr("[预览模式]", "[dry-run]"),
                report.summary(),
                tr("（加 -x 真正执行）", " (add -x to apply)")
            );
        } else {
            println!("\n{}", report.summary());
        }
    }
    if report.errors.is_empty() { ExitCode::SUCCESS } else { ExitCode::from(1) }
}

fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    let json = cli.json;
    match cli.command {
        Command::Crop { io, left, top, right, bottom } => {
            let (files, out) = io.resolve()?;
            let spec = CropSpec { left, top, right, bottom };
            spec.validate().map_err(anyhow::Error::msg)?;
            Ok(print_report(&crop_files(&files, &spec, &out), json))
        }
        Command::Adjust { io, brightness, contrast, saturation, sharpness, grayscale } => {
            let (files, out) = io.resolve()?;
            let spec = AdjustSpec { brightness, contrast, saturation, sharpness, grayscale };
            spec.validate().map_err(anyhow::Error::msg)?;
            Ok(print_report(&adjust_files(&files, &spec, &out), json))
        }
        Command::Enhance { io, mode, no_deskew, max_deskew, no_denoise, only_documents } => {
            let (files, out) = io.resolve()?;
            let mode = match mode.as_str() {
                "color" => EnhanceMode::Color,
                "gray" => EnhanceMode::Gray,
                "bw" => EnhanceMode::Bw,
                other => bail!("{} {other} (color/gray/bw)", tr("未知模式", "Unknown mode")),
            };
            let spec = EnhanceSpec {
                mode,
                deskew: !no_deskew,
                max_deskew_deg: max_deskew,
                denoise: !no_denoise,
                only_documents,
            };
            Ok(print_report(&enhance_files(&files, &spec, &out), json))
        }
        Command::Rename { folder, map, execute, undo } => {
            let (mapping, parse_errors) = if let Some(undo_path) = &undo {
                let log = pic_core::read_undo_log(undo_path)?;
                (pic_core::rename::invert(&log), Vec::new())
            } else {
                let map = map.context(tr(
                    "需要 --map 映射文件（或 --undo 撤销日志）",
                    "--map mapping file required (or --undo log)",
                ))?;
                pic_core::read_mapping_file(&map)?
            };
            for e in &parse_errors {
                eprintln!("⚠ {e}");
            }
            let (report, applied) = pic_core::batch_rename(&folder, &mapping, execute)?;
            if execute && !applied.is_empty() && undo.is_none() {
                match pic_core::write_undo_log(&folder, &applied) {
                    Ok(p) => eprintln!(
                        "{}: {}",
                        tr("undo 日志（可用 --undo 撤销）", "undo log (revert with --undo)"),
                        p.display()
                    ),
                    Err(e) => eprintln!("⚠ {}: {e}", tr("undo 日志写入失败", "failed to write undo log")),
                }
            }
            Ok(print_report(&report, json))
        }
        Command::Ls { folder } => {
            let files = list_images(&folder)?;
            for f in &files {
                println!("{}", f.file_name().unwrap_or_default().to_string_lossy());
            }
            eprintln!("{} {}", tr("图片数量:", "images:"), files.len());
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn main() -> ExitCode {
    pic_core::init_locale_from_env();
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{}: {e}", tr("错误", "error"));
            ExitCode::from(2)
        }
    }
}
