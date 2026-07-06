use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Report {
    pub dry_run: bool,
    pub ok: Vec<Entry>,
    pub skipped: Vec<Entry>,
    pub errors: Vec<Entry>,
}

impl Report {
    pub fn push_ok(&mut self, file: impl Into<String>, output: Option<String>, detail: Option<String>) {
        self.ok.push(Entry { file: file.into(), output, detail });
    }

    pub fn push_skip(&mut self, file: impl Into<String>, reason: impl Into<String>) {
        self.skipped.push(Entry { file: file.into(), output: None, detail: Some(reason.into()) });
    }

    pub fn push_err(&mut self, file: impl Into<String>, error: impl Into<String>) {
        self.errors.push(Entry { file: file.into(), output: None, detail: Some(error.into()) });
    }

    pub fn summary(&self) -> String {
        format!(
            "{} {}, {} {}, {} {}",
            crate::locale::tr("成功", "ok:"),
            self.ok.len(),
            crate::locale::tr("跳过", "skipped:"),
            self.skipped.len(),
            crate::locale::tr("失败", "failed:"),
            self.errors.len()
        )
    }
}
