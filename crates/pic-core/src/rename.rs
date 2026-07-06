use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::io::PicError;
use crate::locale::tr;
use crate::report::Report;

const WINDOWS_FORBIDDEN: &[char] = &['\\', '/', ':', '*', '?', '"', '<', '>', '|'];
const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliedRename {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoLog {
    pub folder: String,
    pub renames: Vec<AppliedRename>,
}

/// 解析映射文本：每行 `旧名,新名`，支持半角/全角逗号和 Tab，# 开头为注释。
/// 返回 (映射对, 解析错误)。
pub fn parse_mapping(text: &str) -> (Vec<(String, String)>, Vec<String>) {
    let mut pairs = Vec::new();
    let mut errors = Vec::new();
    for (lineno, raw) in text.lines().enumerate() {
        let line = raw.trim_start_matches('\u{feff}').trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let split = ['\t', '，', ','].iter().find_map(|sep| {
            line.split_once(*sep)
        });
        let Some((old, new)) = split else {
            errors.push(format!(
                "{} {}: {}",
                tr("第", "Line"),
                lineno + 1,
                tr("行缺少分隔符（逗号或 Tab）", "missing separator (comma or tab)")
            ));
            continue;
        };
        let (old, new) = (old.trim(), new.trim());
        if old.is_empty() || new.is_empty() {
            errors.push(format!(
                "{} {}: {}",
                tr("第", "Line"),
                lineno + 1,
                tr("行缺少旧文件名或新文件名", "missing old or new file name")
            ));
            continue;
        }
        pairs.push((old.to_string(), new.to_string()));
    }
    (pairs, errors)
}

/// 读映射文件：UTF-8（自动剥 BOM）+ UTF-16 LE/BE（按 BOM 探测，兼容 Excel 导出）。
pub fn read_mapping_file(path: &Path) -> Result<(Vec<(String, String)>, Vec<String>), PicError> {
    let bytes = fs::read(path)?;
    let text = decode_text(&bytes).ok_or_else(|| {
        PicError::Msg(format!(
            "{}: {}",
            tr("无法解码映射文件（支持 UTF-8 / UTF-16）", "Cannot decode mapping file (UTF-8 / UTF-16 supported)"),
            path.display()
        ))
    })?;
    Ok(parse_mapping(&text))
}

fn decode_text(bytes: &[u8]) -> Option<String> {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let units: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        return String::from_utf16(&units).ok();
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        return String::from_utf16(&units).ok();
    }
    let start = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) { 3 } else { 0 };
    String::from_utf8(bytes[start..].to_vec()).ok()
}

fn validate_new_name(name: &str) -> Result<(), String> {
    if let Some(bad) = name.chars().find(|c| WINDOWS_FORBIDDEN.contains(c) || (*c as u32) < 32) {
        return Err(format!("{} {bad:?}", tr("新文件名含非法字符", "New name contains invalid character")));
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Err(tr("新文件名不能以句点或空格结尾", "New name cannot end with a dot or space"));
    }
    if name.chars().count() > 255 {
        return Err(tr("新文件名超过 255 字符", "New name exceeds 255 characters"));
    }
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    if WINDOWS_RESERVED.contains(&stem.as_str()) {
        return Err(format!("{stem} {}", tr("是 Windows 保留设备名", "is a reserved Windows device name")));
    }
    Ok(())
}

fn ext_of(name: &str) -> Option<&str> {
    let after_first = &name[name.chars().next().map_or(0, |c| c.len_utf8())..];
    after_first.rfind('.').map(|i| &after_first[i + 1..]).filter(|e| !e.is_empty())
}

/// 按映射批量改名。execute=false 时只做校验和预览（dry-run）。
/// 两阶段改名：先全部改临时名再改目标名，天然支持 a↔b 互换、链式改名、仅大小写改名。
/// 返回 (报告, 实际执行的改名列表——用于写 undo 日志)。
pub fn batch_rename(
    folder: &Path,
    mapping: &[(String, String)],
    execute: bool,
) -> Result<(Report, Vec<AppliedRename>), PicError> {
    if !folder.is_dir() {
        return Err(crate::io::dir_not_found(folder));
    }
    let mut report = Report { dry_run: !execute, ..Default::default() };

    let existing: Vec<String> = fs::read_dir(folder)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    let existing_lower: std::collections::HashSet<String> =
        existing.iter().map(|n| n.to_lowercase()).collect();
    let sources_lower: std::collections::HashSet<String> =
        mapping.iter().map(|(old, _)| old.to_lowercase()).collect();

    let mut taken: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut plan: Vec<(PathBuf, PathBuf, String, String)> = Vec::new();

    for (old, new) in mapping {
        if old.contains(['\\', '/']) {
            report.push_err(
                old.clone(),
                tr("旧文件名不能包含路径分隔符", "Old name cannot contain path separators"),
            );
            continue;
        }
        let src = folder.join(old);
        if !src.is_file() {
            report.push_err(old.clone(), tr("文件不存在", "File not found"));
            continue;
        }
        let mut new_name = new.clone();
        if ext_of(new).is_none() {
            if let Some(e) = ext_of(old) {
                new_name = format!("{new}.{e}");
            }
        }
        if let Err(e) = validate_new_name(&new_name) {
            report.push_err(old.clone(), e);
            continue;
        }
        if new_name == *old {
            report.push_skip(old.clone(), tr("新旧文件名相同", "New name is same as old"));
            continue;
        }
        let new_lower = new_name.to_lowercase();
        if let Some(prev) = taken.get(&new_lower) {
            report.push_err(
                old.clone(),
                format!(
                    "{new_name} {} {prev}",
                    tr("与另一条映射目标重复，来源", "duplicates the target of")
                ),
            );
            continue;
        }
        // 目标已存在且不是本批次要改走的文件、也不是自己换大小写 → 冲突
        if existing_lower.contains(&new_lower)
            && !sources_lower.contains(&new_lower)
            && new_lower != old.to_lowercase()
        {
            report.push_err(
                old.clone(),
                format!("{}: {new_name}", tr("目标文件已存在", "Target file already exists")),
            );
            continue;
        }
        taken.insert(new_lower, old.clone());
        plan.push((src, folder.join(&new_name), old.clone(), new_name));
    }

    if !execute {
        for (_, _, old, new_name) in &plan {
            report.push_ok(old.clone(), Some(new_name.clone()), Some(tr("预览", "preview")));
        }
        return Ok((report, Vec::new()));
    }

    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let mut temps: Vec<(PathBuf, PathBuf, PathBuf, String, String)> = Vec::new();
    for (i, (src, dst, old, new_name)) in plan.into_iter().enumerate() {
        let tmp = folder.join(format!(".pic-rename-{nanos:x}-{i}"));
        match fs::rename(&src, &tmp) {
            Ok(()) => temps.push((tmp, src, dst, old, new_name)),
            Err(e) => report.push_err(old, format!("{}: {e}", tr("改名失败", "Rename failed"))),
        }
    }
    let mut applied = Vec::new();
    for (tmp, src, dst, old, new_name) in temps {
        match fs::rename(&tmp, &dst) {
            Ok(()) => {
                report.push_ok(old.clone(), Some(new_name.clone()), None);
                applied.push(AppliedRename { from: old, to: new_name });
            }
            Err(e) => {
                let _ = fs::rename(&tmp, &src);
                report.push_err(
                    old,
                    format!("{}: {e}", tr("改名失败（已回滚）", "Rename failed (rolled back)")),
                );
            }
        }
    }
    Ok((report, applied))
}

/// 每次真实执行后写 undo 日志，`pic rename --undo <日志>` 可整批撤销。
pub fn write_undo_log(folder: &Path, applied: &[AppliedRename]) -> Result<PathBuf, PicError> {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let path = folder.join(format!("pic-rename-undo-{nanos}.json"));
    let log = UndoLog { folder: folder.to_string_lossy().to_string(), renames: applied.to_vec() };
    fs::write(&path, serde_json::to_string_pretty(&log).map_err(|e| PicError::Msg(e.to_string()))?)?;
    Ok(path)
}

pub fn read_undo_log(path: &Path) -> Result<UndoLog, PicError> {
    let text = fs::read_to_string(path)?;
    serde_json::from_str(&text)
        .map_err(|e| PicError::Msg(format!("{}: {e}", tr("undo 日志解析失败", "Failed to parse undo log"))))
}

/// 反转 undo 日志为映射（to → from），用同一套 batch_rename 执行撤销。
pub fn invert(log: &UndoLog) -> Vec<(String, String)> {
    log.renames.iter().map(|r| (r.to.clone(), r.from.clone())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_separators_and_bom() {
        let text = "\u{feff}1.png,张三.png\na.png，李四.png\nb.png\t王五.png\n# 注释\n\nbad-line\n";
        let (pairs, errors) = parse_mapping(text);
        assert_eq!(pairs.len(), 3);
        assert_eq!(pairs[0], ("1.png".into(), "张三.png".into()));
        assert_eq!(pairs[1], ("a.png".into(), "李四.png".into()));
        assert_eq!(pairs[2], ("b.png".into(), "王五.png".into()));
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn utf16_le_decoding() {
        let text = "1.png,张三.png";
        let mut bytes = vec![0xFF, 0xFE];
        for u in text.encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        let decoded = decode_text(&bytes).unwrap();
        assert_eq!(decoded, text);
    }

    #[test]
    fn reserved_and_forbidden_names() {
        assert!(validate_new_name("CON.png").is_err());
        assert!(validate_new_name("a:b.png").is_err());
        assert!(validate_new_name("bad.").is_err());
        assert!(validate_new_name("正常名.png").is_ok());
    }

    fn setup(files: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for f in files {
            fs::write(dir.path().join(f), b"x").unwrap();
        }
        dir
    }

    #[test]
    fn rename_with_auto_extension() {
        let dir = setup(&["1.png", "a.png"]);
        let mapping = vec![("1.png".into(), "张三".into()), ("a.png".into(), "李四.png".into())];
        let (report, applied) = batch_rename(dir.path(), &mapping, true).unwrap();
        assert_eq!(report.ok.len(), 2, "{report:?}");
        assert!(dir.path().join("张三.png").is_file());
        assert!(dir.path().join("李四.png").is_file());
        assert_eq!(applied.len(), 2);
    }

    #[test]
    fn swap_two_files() {
        let dir = setup(&[]);
        fs::write(dir.path().join("a.png"), b"AAA").unwrap();
        fs::write(dir.path().join("b.png"), b"BBB").unwrap();
        let mapping = vec![("a.png".into(), "b.png".into()), ("b.png".into(), "a.png".into())];
        let (report, _) = batch_rename(dir.path(), &mapping, true).unwrap();
        assert_eq!(report.ok.len(), 2, "{report:?}");
        assert_eq!(fs::read(dir.path().join("b.png")).unwrap(), b"AAA");
        assert_eq!(fs::read(dir.path().join("a.png")).unwrap(), b"BBB");
    }

    #[test]
    fn dry_run_touches_nothing() {
        let dir = setup(&["1.png"]);
        let mapping = vec![("1.png".into(), "新名.png".into())];
        let (report, applied) = batch_rename(dir.path(), &mapping, false).unwrap();
        assert!(report.dry_run);
        assert_eq!(report.ok.len(), 1);
        assert!(applied.is_empty());
        assert!(dir.path().join("1.png").is_file());
        assert!(!dir.path().join("新名.png").exists());
    }

    #[test]
    fn conflicts_detected() {
        let dir = setup(&["1.png", "2.png", "occupied.png"]);
        let mapping = vec![
            ("1.png".into(), "same.png".into()),
            ("2.png".into(), "same.png".into()),
            ("missing.png".into(), "x.png".into()),
            ("2.png".into(), "occupied.png".into()),
        ];
        let (report, _) = batch_rename(dir.path(), &mapping, false).unwrap();
        // 重复目标、源不存在、目标已存在 → 3 个错误；1.png -> same.png 可行
        assert_eq!(report.ok.len(), 1, "{report:?}");
        assert_eq!(report.errors.len(), 3, "{report:?}");
    }

    #[test]
    fn undo_roundtrip() {
        let dir = setup(&["1.png"]);
        let mapping = vec![("1.png".into(), "改后.png".into())];
        let (_, applied) = batch_rename(dir.path(), &mapping, true).unwrap();
        let log_path = write_undo_log(dir.path(), &applied).unwrap();
        let log = read_undo_log(&log_path).unwrap();
        let (report, _) = batch_rename(dir.path(), &invert(&log), true).unwrap();
        assert_eq!(report.ok.len(), 1);
        assert!(dir.path().join("1.png").is_file());
    }

    #[test]
    fn chain_rename() {
        let dir = setup(&[]);
        fs::write(dir.path().join("a.png"), b"A").unwrap();
        fs::write(dir.path().join("b.png"), b"B").unwrap();
        let mapping = vec![("a.png".into(), "b.png".into()), ("b.png".into(), "c.png".into())];
        let (report, _) = batch_rename(dir.path(), &mapping, true).unwrap();
        assert_eq!(report.ok.len(), 2, "{report:?}");
        assert_eq!(fs::read(dir.path().join("b.png")).unwrap(), b"A");
        assert_eq!(fs::read(dir.path().join("c.png")).unwrap(), b"B");
    }
}
