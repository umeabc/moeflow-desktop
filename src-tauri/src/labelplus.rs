//! LabelPlus translation-text generation.
//!
//! A byte-exact port of the server-side generator. The server has two equivalents that
//! must agree with each other and with us:
//!   - `Project.to_labelplus()`  (app/models/project.py) — writes the header, then each file
//!   - `File.to_labelplus()`     (app/models/file.py)    — writes one file's path line + labels
//!
//! Byte-exactness matters: this text is consumed by the LabelPlus Photoshop script, and
//! any drift in line endings or coordinate formatting silently misplaces labels.

use crate::pyfloat::py_float_str;

/// Locale of the three header strings. The server resolves them through `gettext`, so the
/// value depends on the `Accept-Language` of the requesting session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LpLocale {
    Zh,
    En,
}

impl LpLocale {
    /// Map a BCP-47-ish tag to the catalog we actually ship strings for.
    pub fn from_tag(tag: &str) -> Self {
        if tag.to_ascii_lowercase().starts_with("zh") {
            Self::Zh
        } else {
            Self::En
        }
    }

    /// (in-box group, out-of-box group, comment line) — verbatim from the server's `.po` catalogs.
    fn strings(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Zh => (
                "框内",
                "框外",
                "可使用 LabelPlus Photoshop 脚本导入 psd 中",
            ),
            Self::En => (
                "In the box.",
                "Out of the box",
                "You can use the LabelPlus Photoshop script to import into PSD.",
            ),
        }
    }
}

/// One label: a point (not a box) plus its resolved translation text.
#[derive(Debug, Clone)]
pub struct LpSource {
    pub x: f64,
    pub y: f64,
    /// `Source.position_type` — becomes the LabelPlus group id.
    pub position_type: i64,
    /// Already-resolved best translation; `""` when untranslated.
    pub content: String,
}

/// One image file and its labels, in server rank order.
#[derive(Debug, Clone)]
pub struct LpFile {
    /// Ancestor folder names, outermost first. Empty for files at the project root.
    pub dir: Vec<String>,
    pub name: String,
    pub sources: Vec<LpSource>,
}

/// The document header: version line, group-id block, comment block.
pub fn render_header(locale: LpLocale) -> String {
    let (inside, outside, note) = locale.strings();
    format!(
        "1,0\r\n-\r\n{}\r\n{}\r\n-\r\n{}\r\n",
        inside, outside, note
    )
}

/// One `>>>>>>>>[path]<<<<<<<<` line followed by every label block for that file.
pub fn render_file(file: &LpFile) -> String {
    let mut out = String::new();

    let path = if file.dir.is_empty() {
        String::new()
    } else {
        format!("{}/", file.dir.join("/"))
    };
    out.push_str(&format!(">>>>>>>>[{}{}]<<<<<<<<\r\n", path, file.name));

    for (index, source) in file.sources.iter().enumerate() {
        out.push_str(&format!(
            "----------------[{}]----------------[{},{},{}]\r\n",
            index + 1,
            py_float_str(source.x),
            py_float_str(source.y),
            source.position_type,
        ));
        // The server does a naive `\n` -> `\r\n` replacement, so content that already
        // contains CRLF gains an extra CR. Replicate that exactly rather than normalizing.
        out.push_str(&source.content.replace('\n', "\r\n"));
        out.push_str("\r\n");
    }

    out
}

/// The complete `translations.txt` for a project.
pub fn render_document(files: &[LpFile], locale: LpLocale) -> String {
    let mut out = render_header(locale);
    for file in files {
        out.push_str(&render_file(file));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(x: f64, y: f64, pt: i64, content: &str) -> LpSource {
        LpSource {
            x,
            y,
            position_type: pt,
            content: content.to_string(),
        }
    }

    #[test]
    fn header_is_crlf_and_matches_server_layout() {
        let h = render_header(LpLocale::Zh);
        assert_eq!(h, "1,0\r\n-\r\n框内\r\n框外\r\n-\r\n可使用 LabelPlus Photoshop 脚本导入 psd 中\r\n");
    }

    #[test]
    fn english_header_uses_po_catalog_values() {
        let h = render_header(LpLocale::En);
        assert!(h.contains("In the box.\r\n"));
        assert!(h.contains("Out of the box\r\n"));
    }

    #[test]
    fn file_line_and_labels() {
        let file = LpFile {
            dir: vec![],
            name: "001.png".into(),
            sources: vec![src(0.5, 0.25, 1, "你好"), src(0.125, 0.75, 2, "")],
        };
        let out = render_file(&file);
        assert_eq!(
            out,
            ">>>>>>>>[001.png]<<<<<<<<\r\n\
             ----------------[1]----------------[0.5,0.25,1]\r\n\
             你好\r\n\
             ----------------[2]----------------[0.125,0.75,2]\r\n\
             \r\n"
        );
    }

    #[test]
    fn nested_file_gets_directory_prefix() {
        let file = LpFile {
            dir: vec!["第1话".into(), "彩页".into()],
            name: "002.jpg".into(),
            sources: vec![],
        };
        assert!(render_file(&file).starts_with(">>>>>>>>[第1话/彩页/002.jpg]<<<<<<<<\r\n"));
    }

    /// The server replaces `\n` with `\r\n` naively — existing CRLF therefore becomes CRCRLF.
    #[test]
    fn line_ending_normalization_is_naive() {
        let file = LpFile {
            dir: vec![],
            name: "a.png".into(),
            sources: vec![src(0.0, 0.0, 1, "one\ntwo")],
        };
        assert!(render_file(&file).contains("one\r\ntwo\r\n"));
    }

    #[test]
    fn untranslated_label_still_emits_a_blank_line() {
        let file = LpFile {
            dir: vec![],
            name: "a.png".into(),
            sources: vec![src(0.0, 0.0, 1, "")],
        };
        let out = render_file(&file);
        // label line, then an empty content line
        assert!(out.ends_with("----------------[1]----------------[0.0,0.0,1]\r\n\r\n"));
    }

    /// Label numbering restarts at 1 per file and follows rank order, not source id.
    #[test]
    fn label_indices_are_per_file_and_restart() {
        let f1 = LpFile {
            dir: vec![],
            name: "a.png".into(),
            sources: vec![src(0.1, 0.1, 1, "x"), src(0.2, 0.2, 1, "y")],
        };
        let f2 = LpFile {
            dir: vec![],
            name: "b.png".into(),
            sources: vec![src(0.3, 0.3, 1, "z")],
        };
        let doc = render_document(&[f1, f2], LpLocale::Zh);
        assert!(doc.contains("----------------[1]----------------[0.1,0.1,1]\r\n"));
        assert!(doc.contains("----------------[2]----------------[0.2,0.2,1]\r\n"));
        // second file restarts at 1
        assert!(doc.contains(">>>>>>>>[b.png]<<<<<<<<\r\n----------------[1]----------------[0.3,0.3,1]\r\n"));
    }
}
