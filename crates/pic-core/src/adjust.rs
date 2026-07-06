use std::path::PathBuf;

use image::{DynamicImage, Rgba, RgbaImage};
use serde::{Deserialize, Serialize};

use crate::io::{run_batch, OpResult, OutputMode};
use crate::report::Report;

/// 各系数 1.0 = 不变；0.8 = 降低 20%；1.2 = 提高 20%。语义对齐 Pillow ImageEnhance。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct AdjustSpec {
    pub brightness: f32,
    pub contrast: f32,
    pub saturation: f32,
    pub sharpness: f32,
    pub grayscale: bool,
}

impl Default for AdjustSpec {
    fn default() -> Self {
        Self { brightness: 1.0, contrast: 1.0, saturation: 1.0, sharpness: 1.0, grayscale: false }
    }
}

impl AdjustSpec {
    pub fn validate(&self) -> Result<(), String> {
        for (name, v) in [
            ("brightness", self.brightness),
            ("contrast", self.contrast),
            ("saturation", self.saturation),
            ("sharpness", self.sharpness),
        ] {
            if !(0.0..=10.0).contains(&v) {
                return Err(format!(
                    "{name} {} {v}",
                    crate::locale::tr(
                        "需在 0~10 之间（1.0 表示不变），当前",
                        "must be within 0~10 (1.0 = unchanged), got"
                    )
                ));
            }
        }
        if self.is_noop() {
            return Err(crate::locale::tr(
                "所有参数均为默认值，无需处理",
                "All parameters are defaults, nothing to do",
            ));
        }
        Ok(())
    }

    pub fn is_noop(&self) -> bool {
        self.brightness == 1.0
            && self.contrast == 1.0
            && self.saturation == 1.0
            && self.sharpness == 1.0
            && !self.grayscale
    }
}

#[inline]
fn luma(p: &Rgba<u8>) -> f32 {
    0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
}

#[inline]
fn clamp_u8(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

fn blend_channels(img: &mut RgbaImage, f: impl Fn(&Rgba<u8>, usize) -> f32) {
    for p in img.pixels_mut() {
        let src = *p;
        for c in 0..3 {
            p[c] = clamp_u8(f(&src, c));
        }
    }
}

/// Pillow SMOOTH 核（中心 5、其余 1，/13），边缘像素保持原样。
fn smooth(img: &RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    let mut out = img.clone();
    if w < 3 || h < 3 {
        return out;
    }
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let mut acc = [0.0f32; 3];
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let weight = if dx == 0 && dy == 0 { 5.0 } else { 1.0 };
                    let q = img.get_pixel((x as i32 + dx) as u32, (y as i32 + dy) as u32);
                    for c in 0..3 {
                        acc[c] += weight * q[c] as f32;
                    }
                }
            }
            let p = out.get_pixel_mut(x, y);
            for c in 0..3 {
                p[c] = clamp_u8(acc[c] / 13.0);
            }
        }
    }
    out
}

pub fn adjust_image(img: &DynamicImage, spec: &AdjustSpec) -> DynamicImage {
    let mut buf = img.to_rgba8();

    if spec.brightness != 1.0 {
        let f = spec.brightness;
        blend_channels(&mut buf, |p, c| p[c] as f32 * f);
    }
    if spec.contrast != 1.0 {
        let mean = {
            let sum: f64 = buf.pixels().map(|p| luma(p) as f64).sum();
            (sum / (buf.width() as f64 * buf.height() as f64) + 0.5).floor() as f32
        };
        let f = spec.contrast;
        blend_channels(&mut buf, |p, c| mean + (p[c] as f32 - mean) * f);
    }
    if spec.saturation != 1.0 {
        let f = spec.saturation;
        blend_channels(&mut buf, |p, c| {
            let l = luma(p);
            l + (p[c] as f32 - l) * f
        });
    }
    if spec.sharpness != 1.0 {
        let smoothed = smooth(&buf);
        let f = spec.sharpness;
        let (w, h) = buf.dimensions();
        for y in 0..h {
            for x in 0..w {
                let s = smoothed.get_pixel(x, y);
                let p = buf.get_pixel_mut(x, y);
                for c in 0..3 {
                    p[c] = clamp_u8(s[c] as f32 + (p[c] as f32 - s[c] as f32) * f);
                }
            }
        }
    }
    if spec.grayscale {
        blend_channels(&mut buf, |p, _| luma(p));
    }

    if img.color().has_alpha() {
        DynamicImage::ImageRgba8(buf)
    } else {
        DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(buf).to_rgb8())
    }
}

pub fn adjust_files(files: &[PathBuf], spec: &AdjustSpec, out: &OutputMode) -> Report {
    if let Err(e) = spec.validate() {
        let mut r = Report::default();
        r.push_err("<参数>", e);
        return r;
    }
    run_batch(files, out, |img| Ok(OpResult::Done(adjust_image(img, spec), None)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn sample() -> DynamicImage {
        let mut img = RgbImage::new(10, 10);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = Rgb([(x * 20) as u8, (y * 20) as u8, 128]);
        }
        DynamicImage::ImageRgb8(img)
    }

    #[test]
    fn brightness_doubles() {
        let img = DynamicImage::ImageRgb8(RgbImage::from_pixel(4, 4, Rgb([50, 100, 200])));
        let out = adjust_image(&img, &AdjustSpec { brightness: 2.0, ..Default::default() });
        let p = out.to_rgb8().get_pixel(0, 0).0;
        assert_eq!(p, [100, 200, 255]);
    }

    #[test]
    fn grayscale_flattens_channels() {
        let out = adjust_image(&sample(), &AdjustSpec { grayscale: true, ..Default::default() });
        let rgb = out.to_rgb8();
        let p = rgb.get_pixel(3, 7).0;
        assert_eq!(p[0], p[1]);
        assert_eq!(p[1], p[2]);
    }

    #[test]
    fn contrast_zero_goes_to_mean() {
        let out = adjust_image(&sample(), &AdjustSpec { contrast: 0.0, ..Default::default() });
        let rgb = out.to_rgb8();
        let a = rgb.get_pixel(0, 0).0;
        let b = rgb.get_pixel(9, 9).0;
        assert_eq!(a, b);
    }

    #[test]
    fn noop_detected() {
        assert!(AdjustSpec::default().validate().is_err());
    }
}
