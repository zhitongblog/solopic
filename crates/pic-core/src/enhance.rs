use std::path::PathBuf;

use image::imageops::{self, FilterType};
use image::{DynamicImage, GrayImage, Luma, Rgb, RgbImage};
use imageproc::contrast::otsu_level;
use imageproc::filter::{gaussian_blur_f32, median_filter};
use imageproc::geometric_transformations::{rotate_about_center, Interpolation};
use serde::{Deserialize, Serialize};

use crate::io::{run_batch, OpResult, OutputMode};
use crate::report::Report;

/// 智能文档增强：去阴影/光照均衡 + 增白提字 + 自动纠斜 + 锐化。
/// 经典 CV 管线，无模型依赖，单张毫秒级。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct EnhanceSpec {
    pub mode: EnhanceMode,
    /// 自动检测并矫正文字倾斜
    pub deskew: bool,
    /// 纠斜搜索的最大角度（度），实际取值范围 1~45；角度越大误判风险越高、旋转白边越大
    pub max_deskew_deg: f32,
    /// 轻度去噪（黑白模式为中值滤波+去孤立噪点）
    pub denoise: bool,
    /// 仅处理"看起来像文档"的图片，其余跳过（防止风景/人像被误伤）
    pub only_documents: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnhanceMode {
    /// 彩色增强：白底提亮、文字加深、色彩保留（"魔法滤镜"）
    Color,
    /// 灰度增强
    Gray,
    /// 黑白文档：Sauvola 自适应二值化，输出纯白底黑字
    Bw,
}

impl Default for EnhanceSpec {
    fn default() -> Self {
        Self {
            mode: EnhanceMode::Color,
            deskew: true,
            max_deskew_deg: 20.0,
            denoise: true,
            only_documents: false,
        }
    }
}

const WHITE_POINT: f32 = 235.0;
const DESKEW_MIN_DEG: f32 = 0.25;

#[inline]
fn luma_rgb(p: &Rgb<u8>) -> f32 {
    0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
}

fn to_gray(rgb: &RgbImage) -> GrayImage {
    let mut g = GrayImage::new(rgb.width(), rgb.height());
    for (s, d) in rgb.pixels().zip(g.pixels_mut()) {
        d[0] = luma_rgb(s).round() as u8;
    }
    g
}

/// 分离式滑动窗口极值滤波（膨胀 dilate=max / 腐蚀 erode=min），用于小图上估计背景。
fn extremum_filter(img: &GrayImage, radius: u32, max: bool) -> GrayImage {
    let (w, h) = img.dimensions();
    let pick = |a: u8, b: u8| if max { a.max(b) } else { a.min(b) };
    let mut horiz = GrayImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let lo = x.saturating_sub(radius);
            let hi = (x + radius).min(w - 1);
            let mut v = img.get_pixel(lo, y)[0];
            for xx in lo + 1..=hi {
                v = pick(v, img.get_pixel(xx, y)[0]);
            }
            horiz.put_pixel(x, y, Luma([v]));
        }
    }
    let mut out = GrayImage::new(w, h);
    for x in 0..w {
        for y in 0..h {
            let lo = y.saturating_sub(radius);
            let hi = (y + radius).min(h - 1);
            let mut v = horiz.get_pixel(x, lo)[0];
            for yy in lo + 1..=hi {
                v = pick(v, horiz.get_pixel(x, yy)[0]);
            }
            out.put_pixel(x, y, Luma([v]));
        }
    }
    out
}

/// 形态学闭运算：抹掉深色文字，留下光照/纸面背景场。
fn morph_close(img: &GrayImage, radius: u32) -> GrayImage {
    extremum_filter(&extremum_filter(img, radius, true), radius, false)
}

/// 在缩小图上估计单通道背景，再放大回原尺寸。
fn estimate_background(channel: &GrayImage) -> GrayImage {
    let (w, h) = channel.dimensions();
    let short = w.min(h).max(1);
    let scale = (short as f32 / 256.0).max(1.0);
    let (sw, sh) = (
        ((w as f32 / scale) as u32).max(8),
        ((h as f32 / scale) as u32).max(8),
    );
    let small = imageops::resize(channel, sw, sh, FilterType::Triangle);
    let radius = (sw.min(sh) / 16).max(3);
    let closed = morph_close(&small, radius);
    let smoothed = gaussian_blur_f32(&closed, 2.0);
    imageops::resize(&smoothed, w, h, FilterType::Triangle)
}

/// 除法归一化：像素值除以背景估计，阴影和渐变光照被展平，背景变白。
fn divide_normalize(channel: &GrayImage, bg: &GrayImage) -> GrayImage {
    let mut out = GrayImage::new(channel.width(), channel.height());
    for ((s, b), d) in channel.pixels().zip(bg.pixels()).zip(out.pixels_mut()) {
        let v = s[0] as f32 * 255.0 / (b[0] as f32).max(1.0);
        d[0] = v.round().clamp(0.0, 255.0) as u8;
    }
    out
}

fn percentile(img: &GrayImage, pct: f32) -> u8 {
    let mut hist = [0u64; 256];
    for p in img.pixels() {
        hist[p[0] as usize] += 1;
    }
    let total: u64 = hist.iter().sum();
    let target = (total as f64 * pct as f64 / 100.0) as u64;
    let mut acc = 0u64;
    for (v, n) in hist.iter().enumerate() {
        acc += n;
        if acc >= target {
            return v as u8;
        }
    }
    255
}

/// 对比度拉伸 LUT：暗部（文字）压深，白点以上钳制纯白。
fn stretch_lut(lo: u8) -> [u8; 256] {
    let lo = lo.min(120) as f32;
    let mut lut = [0u8; 256];
    for (i, e) in lut.iter_mut().enumerate() {
        let v = (i as f32 - lo) * 255.0 / (WHITE_POINT - lo);
        *e = v.round().clamp(0.0, 255.0) as u8;
    }
    lut
}

// ---------------------------------------------------------------- Sauvola

fn integrals(img: &GrayImage) -> (Vec<u64>, Vec<u64>) {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let stride = w + 1;
    let mut sum = vec![0u64; stride * (h + 1)];
    let mut sq = vec![0u64; stride * (h + 1)];
    for y in 0..h {
        let mut row = 0u64;
        let mut row_sq = 0u64;
        for x in 0..w {
            let v = img.get_pixel(x as u32, y as u32)[0] as u64;
            row += v;
            row_sq += v * v;
            sum[(y + 1) * stride + x + 1] = sum[y * stride + x + 1] + row;
            sq[(y + 1) * stride + x + 1] = sq[y * stride + x + 1] + row_sq;
        }
    }
    (sum, sq)
}

/// Sauvola 自适应二值化：t = m·(1 + k·(s/R − 1))，对光照不均的文档远优于全局阈值。
pub fn sauvola(img: &GrayImage, window: u32, k: f32) -> GrayImage {
    let (w, h) = (img.width() as i64, img.height() as i64);
    let stride = (w + 1) as usize;
    let (sum, sq) = integrals(img);
    let r = window as i64 / 2;
    let mut out = GrayImage::new(img.width(), img.height());
    for y in 0..h {
        for x in 0..w {
            let x0 = (x - r).max(0) as usize;
            let y0 = (y - r).max(0) as usize;
            let x1 = (x + r + 1).min(w) as usize;
            let y1 = (y + r + 1).min(h) as usize;
            let n = ((x1 - x0) * (y1 - y0)) as f64;
            let s1 = (sum[y1 * stride + x1] + sum[y0 * stride + x0]) as f64
                - (sum[y0 * stride + x1] + sum[y1 * stride + x0]) as f64;
            let s2 = (sq[y1 * stride + x1] + sq[y0 * stride + x0]) as f64
                - (sq[y0 * stride + x1] + sq[y1 * stride + x0]) as f64;
            let mean = s1 / n;
            let var = (s2 / n - mean * mean).max(0.0);
            let t = mean * (1.0 + k as f64 * (var.sqrt() / 128.0 - 1.0));
            let v = img.get_pixel(x as u32, y as u32)[0] as f64;
            out.put_pixel(x as u32, y as u32, Luma([if v < t { 0 } else { 255 }]));
        }
    }
    out
}

/// 去孤立噪点：黑点周围 8 邻域全白则抹白。
fn despeckle(img: &mut GrayImage) {
    let (w, h) = img.dimensions();
    if w < 3 || h < 3 {
        return;
    }
    let src = img.clone();
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            if src.get_pixel(x, y)[0] != 0 {
                continue;
            }
            let mut neighbors = 0;
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    if (dx, dy) == (0, 0) {
                        continue;
                    }
                    if src.get_pixel((x as i32 + dx) as u32, (y as i32 + dy) as u32)[0] == 0 {
                        neighbors += 1;
                    }
                }
            }
            if neighbors == 0 {
                img.put_pixel(x, y, Luma([255]));
            }
        }
    }
}

// ---------------------------------------------------------------- 纠斜

/// 投影法评分：文本行水平时，各行黑像素计数的平方和最大。
fn projection_score(bin: &GrayImage) -> f64 {
    let (w, h) = bin.dimensions();
    let mut score = 0.0;
    for y in 0..h {
        let mut row = 0u64;
        for x in 0..w {
            if bin.get_pixel(x, y)[0] == 0 {
                row += 1;
            }
        }
        score += (row * row) as f64;
    }
    score
}

/// 估计需要旋转的角度（度），在 ±max_deg 内搜索。返回 None 表示不需要/不敢转。
fn estimate_deskew_angle(gray_norm: &GrayImage, max_deg: f32) -> Option<f32> {
    let max_deg = max_deg.clamp(1.0, 45.0);
    let (w, h) = gray_norm.dimensions();
    let short = w.min(h).max(1);
    let scale = (short as f32 / 400.0).max(1.0);
    let small = imageops::resize(
        gray_norm,
        ((w as f32 / scale) as u32).max(16),
        ((h as f32 / scale) as u32).max(16),
        FilterType::Triangle,
    );
    let thr = otsu_level(&small);
    let mut bin = small;
    for p in bin.pixels_mut() {
        p[0] = if p[0] < thr { 0 } else { 255 };
    }

    let score_at = |deg: f32| -> f64 {
        if deg == 0.0 {
            return projection_score(&bin);
        }
        let rotated = rotate_about_center(&bin, deg.to_radians(), Interpolation::Nearest, Luma([255]));
        projection_score(&rotated)
    };

    let base = score_at(0.0);
    let mut best = (0.0f32, base);
    let mut deg = -max_deg;
    while deg <= max_deg {
        let s = score_at(deg);
        if s > best.1 {
            best = (deg, s);
        }
        deg += 1.0;
    }
    let center = best.0;
    let mut fine = center - 0.9;
    while fine <= center + 0.9 {
        let s = score_at(fine);
        if s > best.1 {
            best = (fine, s);
        }
        fine += 0.15;
    }

    // 提升不明显就不转，避免把非文本图转歪
    if best.0.abs() < DESKEW_MIN_DEG || best.1 < base * 1.05 {
        None
    } else {
        Some(best.0)
    }
}

// ---------------------------------------------------------------- 文档判别

/// 启发式判断是否文档照片：亮背景峰 + 低饱和度占比 + 合理墨水覆盖率，三中二。
pub fn looks_like_document(rgb: &RgbImage) -> bool {
    let (w, h) = rgb.dimensions();
    let short = w.min(h).max(1);
    let scale = (short as f32 / 256.0).max(1.0);
    let small = imageops::resize(rgb, ((w as f32 / scale) as u32).max(8), ((h as f32 / scale) as u32).max(8), FilterType::Triangle);
    let gray = to_gray(&small);
    let total = (gray.width() * gray.height()) as f64;

    let mut hist = [0u64; 256];
    for p in gray.pixels() {
        hist[p[0] as usize] += 1;
    }
    let peak = (140..256).max_by_key(|&i| hist[i]).unwrap_or(255);
    let near_peak: u64 = hist[peak.saturating_sub(25)..(peak + 25).min(256)].iter().sum();
    let bright_bg = hist[peak] > 0 && near_peak as f64 / total > 0.4;

    let low_sat = small
        .pixels()
        .filter(|p| {
            let max = p[0].max(p[1]).max(p[2]);
            let min = p[0].min(p[1]).min(p[2]);
            max - min < 40
        })
        .count() as f64
        / total
        > 0.6;

    let thr = otsu_level(&gray);
    let fg = gray.pixels().filter(|p| p[0] < thr).count() as f64 / total;
    let ink_ratio = fg > 0.003 && fg < 0.35;

    [bright_bg, low_sat, ink_ratio].iter().filter(|&&b| b).count() >= 2
}

// ---------------------------------------------------------------- 主流程

fn unsharp_rgb(img: &RgbImage, amount: f32) -> RgbImage {
    let blurred = gaussian_blur_f32(img, 1.2);
    let mut out = img.clone();
    for ((o, b), d) in img.pixels().zip(blurred.pixels()).zip(out.pixels_mut()) {
        for c in 0..3 {
            let v = o[c] as f32 + (o[c] as f32 - b[c] as f32) * amount;
            d[c] = v.round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

fn boost_saturation(img: &mut RgbImage, f: f32) {
    for p in img.pixels_mut() {
        let l = luma_rgb(p);
        for c in 0..3 {
            let v = l + (p[c] as f32 - l) * f;
            p[c] = v.round().clamp(0.0, 255.0) as u8;
        }
    }
}

pub struct EnhanceOutcome {
    pub image: DynamicImage,
    pub deskewed_deg: Option<f32>,
}

pub fn enhance_image(img: &DynamicImage, spec: &EnhanceSpec) -> EnhanceOutcome {
    let rgb = img.to_rgb8();
    let gray = to_gray(&rgb);

    // 光照均衡（逐通道除法归一化，彩色阴影也能去掉）
    let mut channels: Vec<GrayImage> = Vec::new();
    if spec.mode == EnhanceMode::Color {
        for c in 0..3 {
            let mut ch = GrayImage::new(rgb.width(), rgb.height());
            for (s, d) in rgb.pixels().zip(ch.pixels_mut()) {
                d[0] = s[c];
            }
            let bg = estimate_background(&ch);
            channels.push(divide_normalize(&ch, &bg));
        }
    }
    let gray_bg = estimate_background(&gray);
    let gray_norm = divide_normalize(&gray, &gray_bg);

    let angle =
        if spec.deskew { estimate_deskew_angle(&gray_norm, spec.max_deskew_deg) } else { None };

    let image = match spec.mode {
        EnhanceMode::Color => {
            let mut norm = RgbImage::new(rgb.width(), rgb.height());
            for (i, p) in norm.pixels_mut().enumerate() {
                let x = (i as u32) % rgb.width();
                let y = (i as u32) / rgb.width();
                *p = Rgb([
                    channels[0].get_pixel(x, y)[0],
                    channels[1].get_pixel(x, y)[0],
                    channels[2].get_pixel(x, y)[0],
                ]);
            }
            let lut = stretch_lut(percentile(&gray_norm, 1.0));
            for p in norm.pixels_mut() {
                for c in 0..3 {
                    p[c] = lut[p[c] as usize];
                }
            }
            boost_saturation(&mut norm, 1.25);
            let mut result = unsharp_rgb(&norm, 0.6);
            if let Some(deg) = angle {
                result = rotate_about_center(&result, deg.to_radians(), Interpolation::Bilinear, Rgb([255, 255, 255]));
            }
            DynamicImage::ImageRgb8(result)
        }
        EnhanceMode::Gray => {
            let lut = stretch_lut(percentile(&gray_norm, 1.0));
            let mut g = gray_norm.clone();
            for p in g.pixels_mut() {
                p[0] = lut[p[0] as usize];
            }
            let mut rgb_g = RgbImage::new(g.width(), g.height());
            for (s, d) in g.pixels().zip(rgb_g.pixels_mut()) {
                *d = Rgb([s[0], s[0], s[0]]);
            }
            let sharpened = unsharp_rgb(&rgb_g, 0.6);
            let mut g2 = to_gray(&sharpened);
            if let Some(deg) = angle {
                g2 = rotate_about_center(&g2, deg.to_radians(), Interpolation::Bilinear, Luma([255]));
            }
            DynamicImage::ImageLuma8(g2)
        }
        EnhanceMode::Bw => {
            let mut src = gray_norm.clone();
            if spec.denoise {
                src = median_filter(&src, 1, 1);
            }
            let short = src.width().min(src.height());
            let window = ((short / 32) | 1).clamp(15, 61);
            let mut bin = sauvola(&src, window, 0.2);
            if spec.denoise {
                despeckle(&mut bin);
            }
            if let Some(deg) = angle {
                bin = rotate_about_center(&bin, deg.to_radians(), Interpolation::Bilinear, Luma([255]));
                for p in bin.pixels_mut() {
                    p[0] = if p[0] < 128 { 0 } else { 255 };
                }
            }
            DynamicImage::ImageLuma8(bin)
        }
    };

    EnhanceOutcome { image, deskewed_deg: angle }
}

pub fn enhance_files(files: &[PathBuf], spec: &EnhanceSpec, out: &OutputMode) -> Report {
    run_batch(files, out, |img| {
        if spec.only_documents && !looks_like_document(&img.to_rgb8()) {
            return Ok(OpResult::Skip(crate::locale::tr(
                "判定为非文档图片，已跳过（可关闭\"仅处理文档\"强制处理）",
                "Not detected as a document, skipped (disable \"documents only\" to force)",
            )));
        }
        let outcome = enhance_image(img, spec);
        let detail = outcome
            .deskewed_deg
            .map(|d| format!("{} {d:+.1}°", crate::locale::tr("自动纠斜", "deskewed")));
        Ok(OpResult::Done(outcome.image, detail))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一张"文档照片"：浅灰底 + 从左到右的阴影渐变 + 黑色文字行。
    fn synthetic_doc(w: u32, h: u32, skew_deg: f32) -> RgbImage {
        let mut img = RgbImage::new(w, h);
        for (x, _y, p) in img.enumerate_pixels_mut() {
            let shade = 200.0 - 80.0 * (x as f32 / w as f32);
            *p = Rgb([shade as u8, shade as u8, shade as u8]);
        }
        let rad = skew_deg.to_radians();
        for line in 0..8 {
            let base_y = 40 + line * 40;
            for x in 20..w - 20 {
                let y = base_y as f32 + (x as f32 - w as f32 / 2.0) * rad.tan();
                for dy in 0..6 {
                    let yy = y as i32 + dy;
                    if yy >= 0 && (yy as u32) < h {
                        img.put_pixel(x, yy as u32, Rgb([20, 20, 20]));
                    }
                }
            }
        }
        img
    }

    #[test]
    fn shadow_removed_in_bw_mode() {
        let doc = synthetic_doc(400, 400, 0.0);
        let spec = EnhanceSpec { mode: EnhanceMode::Bw, deskew: false, ..Default::default() };
        let out = enhance_image(&DynamicImage::ImageRgb8(doc), &spec);
        let g = out.image.to_luma8();
        // 阴影区（右侧原本 ~120 灰）的背景应该变成纯白
        let corner = g.get_pixel(g.width() - 10, 10)[0];
        assert_eq!(corner, 255, "阴影背景应被归一化为白色");
        // 文字应该还在（存在黑色像素）
        let black = g.pixels().filter(|p| p[0] == 0).count();
        assert!(black > 1000, "文字应保留，黑像素 {black}");
    }

    #[test]
    fn deskew_detects_synthetic_tilt() {
        let doc = synthetic_doc(600, 600, 3.0);
        let gray = to_gray(&doc);
        let bg = estimate_background(&gray);
        let norm = divide_normalize(&gray, &bg);
        let angle = estimate_deskew_angle(&norm, 12.0).expect("应检测到倾斜");
        assert!((angle.abs() - 3.0).abs() < 1.0, "检测角度 {angle} 应接近 ±3°");
    }

    #[test]
    fn straight_doc_not_rotated() {
        let doc = synthetic_doc(600, 600, 0.0);
        let gray = to_gray(&doc);
        let bg = estimate_background(&gray);
        let norm = divide_normalize(&gray, &bg);
        let angle = estimate_deskew_angle(&norm, 12.0);
        assert!(angle.is_none() || angle.unwrap().abs() < 0.5, "水平文档不应被旋转: {angle:?}");
    }

    #[test]
    fn document_heuristic() {
        let doc = synthetic_doc(400, 400, 0.0);
        assert!(looks_like_document(&doc));
        // 高饱和随机彩色图不应判定为文档
        let mut photo = RgbImage::new(256, 256);
        for (x, y, p) in photo.enumerate_pixels_mut() {
            *p = Rgb([(x * 7 % 256) as u8, (y * 13 % 256) as u8, ((x + y) * 3 % 256) as u8]);
        }
        assert!(!looks_like_document(&photo));
    }

    #[test]
    fn sauvola_handles_gradient() {
        let doc = synthetic_doc(300, 300, 0.0);
        let gray = to_gray(&doc);
        let bin = sauvola(&gray, 31, 0.2);
        let blacks = bin.pixels().filter(|p| p[0] == 0).count();
        let total = (bin.width() * bin.height()) as usize;
        assert!(blacks > total / 100 && blacks < total / 2);
    }
}
