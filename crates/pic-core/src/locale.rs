//! 运行期语言切换：核心引擎消息中英双语，GUI 八语言在前端层实现。

use std::sync::atomic::{AtomicU8, Ordering};

static LOCALE: AtomicU8 = AtomicU8::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locale {
    Zh,
    En,
}

pub fn set_locale(locale: Locale) {
    LOCALE.store(if locale == Locale::Zh { 0 } else { 1 }, Ordering::Relaxed);
}

pub fn locale() -> Locale {
    if LOCALE.load(Ordering::Relaxed) == 0 { Locale::Zh } else { Locale::En }
}

/// 从 PIC_LANG 环境变量（zh/en）或系统语言初始化，各壳启动时调用。
pub fn init_locale_from_env() {
    let lang = std::env::var("PIC_LANG")
        .ok()
        .or_else(|| sys_locale::get_locale())
        .unwrap_or_default()
        .to_lowercase();
    set_locale(if lang.starts_with("zh") { Locale::Zh } else { Locale::En });
}

pub fn set_locale_by_tag(tag: &str) {
    set_locale(if tag.to_lowercase().starts_with("zh") { Locale::Zh } else { Locale::En });
}

pub fn tr(zh: &str, en: &str) -> String {
    match locale() {
        Locale::Zh => zh.to_string(),
        Locale::En => en.to_string(),
    }
}
