//! Local LabelPlus txt + outputs-zip generation.
//!
//! This reproduces, on the client, what `app/tasks/output_project.py` does on the server:
//! it walks the project's image files, resolves each label's best translation, renders
//! `translations.txt`, and packs `images/` + `translations.txt` + `project.json` into a zip.
//!
//! Doing it locally means no Celery round-trip and no server CPU — and images that are
//! already in the media cache cost nothing to include.
//!
//! # The blank-source hazard
//!
//! The server's export iterates *every* source (`File.to_labelplus` → `self.sources()`),
//! but the only source-listing endpoint calls `File.to_translator(..., show_blank=False)`
//! by default, which drops blank sources. Blank sources are common on images: importing a
//! LabelPlus file whose label has no translation creates one.
//!
//! Dropping them would shift every subsequent label index in the output — silently, since
//! the file still looks well-formed. We therefore:
//!   1. send `show_blank=true` (harmless on unpatched servers, correct on patched ones),
//!   2. detect the filtering by looking for gaps in the `rank` sequence, and
//!   3. report it loudly rather than emitting a quietly-wrong file.
//!
//! `patches/backend-show-blank.patch` makes an unpatched server honour the parameter.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

use crate::labelplus::{self, LpFile, LpLocale, LpSource};
use crate::media::MediaCache;

/// `FileType.IMAGE` — see `app/constants/file.py`.
const FILE_TYPE_IMAGE: i64 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportRequest {
    pub project_id: String,
    pub target_id: String,
    /// Bearer token forwarded from the renderer.
    pub token: String,
    /// BCP-47 tag, selects the header language.
    pub locale: String,
    pub destination: PathBuf,
    /// Include the `images/` folder. Turning this off produces a txt-only archive.
    pub include_images: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportReport {
    pub path: String,
    pub file_count: usize,
    pub image_count: usize,
    pub skipped_images: Vec<String>,
    /// Non-fatal problems the user should know about.
    pub warnings: Vec<String>,
    /// True when the export may not match a server-side export byte-for-byte.
    pub diverged: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct ApiFile {
    id: String,
    name: String,
    #[serde(rename = "type")]
    file_type: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct ApiTranslation {
    #[serde(default)]
    content: String,
    #[serde(default)]
    proofread_content: String,
    #[serde(default)]
    selected: bool,
    #[serde(default)]
    edit_time: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ApiSource {
    x: f64,
    y: f64,
    #[serde(default)]
    rank: i64,
    position_type: i64,
    #[serde(default)]
    my_translation: Option<ApiTranslation>,
    #[serde(default)]
    translations: Vec<ApiTranslation>,
}

/// Mirrors `app/models/file.py::Filename._get_sort_name`.
///
/// The server orders files by `(dir_sort_name, type, sort_name)`. `sort_name` left-pads
/// every numeric run in the filename to 6 digits so `2.jpg` sorts before `10.jpg`.
pub fn sort_name(name: &str) -> String {
    let prefix = match name.rsplit_once('.') {
        Some((prefix, _suffix)) => prefix,
        None => name,
    };
    let mut out = String::new();
    for part in split_digit_runs(prefix) {
        if part.chars().all(|c| c.is_numeric()) {
            out.push_str(&"0".repeat(6usize.saturating_sub(part.chars().count())));
            out.push_str(&part);
        } else {
            out.push_str(&part);
        }
    }
    out
}

/// Equivalent of Python's `re.findall(r"\d+|\D+", text)`.
fn split_digit_runs(text: &str) -> Vec<String> {
    let mut runs: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_is_digit: Option<bool> = None;

    for ch in text.chars() {
        let is_digit = ch.is_numeric();
        match current_is_digit {
            Some(previous) if previous == is_digit => current.push(ch),
            Some(_) => {
                runs.push(std::mem::take(&mut current));
                current.push(ch);
                current_is_digit = Some(is_digit);
            }
            None => {
                current.push(ch);
                current_is_digit = Some(is_digit);
            }
        }
    }
    if !current.is_empty() {
        runs.push(current);
    }
    runs
}

/// Pick the translation the server would pick.
///
/// Replicates `default_translations_order = ["-selected", "-proofread_content", "-edit_time"]`
/// and the `proofread_content or content` fallback in `File.to_labelplus`.
fn best_content(sources: &ApiSource) -> String {
    let mut all: Vec<&ApiTranslation> = Vec::new();
    if let Some(mine) = sources.my_translation.as_ref() {
        all.push(mine);
    }
    all.extend(sources.translations.iter());

    all.sort_by(|a, b| {
        b.selected
            .cmp(&a.selected)
            // Descending on the raw string, with the empty value last — MongoDB's
            // behaviour for `-proofread_content`.
            .then_with(|| {
                let a_empty = a.proofread_content.is_empty();
                let b_empty = b.proofread_content.is_empty();
                match (a_empty, b_empty) {
                    (true, false) => std::cmp::Ordering::Greater,
                    (false, true) => std::cmp::Ordering::Less,
                    _ => b.proofread_content.cmp(&a.proofread_content),
                }
            })
            .then_with(|| b.edit_time.cmp(&a.edit_time))
    });

    match all.first() {
        Some(t) if !t.proofread_content.is_empty() => t.proofread_content.clone(),
        Some(t) => t.content.clone(),
        None => String::new(),
    }
}

/// Ranks are assigned densely, so a gap means the server hid blank sources from us.
fn has_rank_gap(sources: &[ApiSource]) -> bool {
    if sources.is_empty() {
        return false;
    }
    let min = sources.iter().map(|s| s.rank).min().unwrap_or(0);
    let max = sources.iter().map(|s| s.rank).max().unwrap_or(0);
    let span = (max - min + 1) as usize;
    span != sources.len()
}

struct Api<'a> {
    client: &'a reqwest::Client,
    api_base: &'a str,
    token: &'a str,
}

impl<'a> Api<'a> {
    async fn get_json<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
    ) -> Result<T, String> {
        let url = format!("{}/{}", self.api_base.trim_end_matches('/'), path);
        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .await
            .map_err(|err| format!("请求 {path} 失败：{err}"))?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err("登录状态已失效，请重新登录".to_string());
        }
        if !status.is_success() {
            return Err(format!("{path} 返回 {status}"));
        }
        response
            .json::<T>()
            .await
            .map_err(|err| format!("解析 {path} 响应失败：{err}"))
    }
}

/// A file plus the ordering keys the server would have computed.
struct Walked {
    id: String,
    name: String,
    /// Ancestor folder names, outermost first.
    dir: Vec<String>,
    dir_sort_name: String,
    sort_name: String,
    file_type: i64,
}

/// Walk the project tree depth-first, computing `dir_sort_name` exactly as `File.save` does.
async fn walk_tree(api: &Api<'_>, project_id: &str) -> Result<Vec<Walked>, String> {
    let mut out = Vec::new();
    let mut stack: Vec<(Option<String>, Vec<String>, String)> =
        vec![(None, Vec::new(), String::new())];

    while let Some((parent_id, dir, dir_sort_name)) = stack.pop() {
        let path = match &parent_id {
            Some(id) => format!("v1/projects/{project_id}/files?parent_id={id}"),
            None => format!("v1/projects/{project_id}/files"),
        };
        let entries: Vec<ApiFile> = api.get_json(&path).await?;

        // Children of a folder sort the same way the server sorts a flat query.
        let mut children: Vec<Walked> = entries
            .into_iter()
            .map(|entry| Walked {
                id: entry.id,
                sort_name: sort_name(&entry.name),
                name: entry.name,
                dir: dir.clone(),
                dir_sort_name: dir_sort_name.clone(),
                file_type: entry.file_type,
            })
            .collect();
        children.sort_by(|a, b| a.sort_name.cmp(&b.sort_name));

        // Push folders in reverse so the stack yields them in sorted order.
        let folders: Vec<&Walked> = children.iter().filter(|c| c.file_type == 1).collect();
        for folder in folders.iter().rev() {
            let mut child_dir = dir.clone();
            child_dir.push(folder.name.clone());
            // `File.save`: parent.dir_sort_name + parent.sort_name + "/"
            let child_dir_sort = format!("{}{}/", folder.dir_sort_name, folder.sort_name);
            stack.push((Some(folder.id.clone()), child_dir, child_dir_sort));
        }

        out.extend(children);
    }

    Ok(out)
}

/// Fetch an image into the media cache (or reuse it) and return its on-disk path.
async fn image_path(
    client: &reqwest::Client,
    api_base: &str,
    cache: &MediaCache,
    token: &str,
    url: &str,
) -> Result<PathBuf, String> {
    if let Some(path) = cache.get(url) {
        return Ok(path);
    }

    let mut request = client.get(url);
    // Storage URLs are normally pre-signed, but a same-origin deployment may still want auth.
    if url.starts_with(api_base) {
        request = request.header("Authorization", format!("Bearer {token}"));
    }

    let response = request.send().await.map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }

    let temp = cache.temp_path();
    let mut file = tokio::fs::File::create(&temp)
        .await
        .map_err(|err| err.to_string())?;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|err| err.to_string())?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
            .await
            .map_err(|err| err.to_string())?;
    }
    drop(file);

    cache
        .commit(url, &temp, None)
        .map_err(|err| err.to_string())
}

/// Run a full local export.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    client: &reqwest::Client,
    api_base: &str,
    cache: Arc<MediaCache>,
    request: &ExportRequest,
    mut progress: impl FnMut(&str, f32),
) -> Result<ExportReport, String> {
    let api = Api {
        client,
        api_base,
        token: &request.token,
    };
    let locale = LpLocale::from_tag(&request.locale);

    progress("读取项目文件列表", 0.02);
    let mut tree = walk_tree(&api, &request.project_id).await?;

    // Same ordering the server applies via `mongo_order(files, None, ["dir_sort_name","type","sort_name"])`.
    tree.sort_by(|a, b| {
        a.dir_sort_name
            .cmp(&b.dir_sort_name)
            .then_with(|| a.file_type.cmp(&b.file_type))
            .then_with(|| a.sort_name.cmp(&b.sort_name))
    });

    let images: Vec<&Walked> = tree.iter().filter(|f| f.file_type == FILE_TYPE_IMAGE).collect();
    let total = images.len().max(1);

    let mut warnings: Vec<String> = Vec::new();
    let mut gap_files: Vec<String> = Vec::new();
    let mut rendered: Vec<LpFile> = Vec::new();
    // name -> source file path; later entries overwrite earlier ones, matching the
    // server's write-into-a-folder-then-zip behaviour.
    let mut image_files: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut skipped_images: Vec<String> = Vec::new();

    for (index, file) in images.iter().enumerate() {
        progress(
            &format!("读取原文 {}/{}", index + 1, total),
            0.05 + 0.75 * (index as f32 / total as f32),
        );

        let path = format!(
            "v1/files/{}/sources?target_id={}&paging=false&show_blank=true",
            file.id, request.target_id
        );
        let sources: Vec<ApiSource> = api.get_json(&path).await?;

        if has_rank_gap(&sources) {
            gap_files.push(file.name.clone());
        }

        rendered.push(LpFile {
            dir: file.dir.clone(),
            name: file.name.clone(),
            sources: sources
                .iter()
                .map(|source| LpSource {
                    x: source.x,
                    y: source.y,
                    position_type: source.position_type,
                    content: best_content(source),
                })
                .collect(),
        });

        if request.include_images {
            // The backend's `File.url` is what lands in the zip; fall back to the
            // file-content endpoint when a storage URL is unavailable.
            let url = format!(
                "{}/v1/files/{}/content",
                api_base.trim_end_matches('/'),
                file.id
            );
            match image_path(client, api_base, &cache, &request.token, &url).await {
                Ok(path) => {
                    image_files.insert(file.name.clone(), path);
                }
                Err(err) => {
                    skipped_images.push(format!("{}（{err}）", file.name));
                }
            }
        }
    }

    if !gap_files.is_empty() {
        warnings.push(format!(
            "有 {} 张图片存在被服务器隐藏的空白原文，导出的标号编号会与服务器结果不一致。",
            gap_files.len()
        ));
    }

    progress("生成 translations.txt", 0.85);
    let text = labelplus::render_document(&rendered, locale);

    progress("打包 zip", 0.9);
    let project_json = build_project_json(&api, request).await;
    write_zip(
        &request.destination,
        &text,
        project_json,
        &image_files,
        &skipped_images,
    )?;

    Ok(ExportReport {
        path: request.destination.to_string_lossy().to_string(),
        file_count: rendered.len(),
        image_count: image_files.len(),
        skipped_images,
        diverged: !gap_files.is_empty(),
        warnings,
    })
}

/// Mirror of `Project.to_output_json()` plus the two fields the task adds.
async fn build_project_json(api: &Api<'_>, request: &ExportRequest) -> serde_json::Value {
    let detail: serde_json::Value = api
        .get_json(&format!("v1/projects/{}", request.project_id))
        .await
        .unwrap_or(serde_json::Value::Null);

    let pick = |key: &str| detail.get(key).cloned().unwrap_or(serde_json::Value::Null);

    serde_json::json!({
        "name": pick("name"),
        "intro": pick("intro"),
        "default_role": pick("default_role"),
        "allow_apply_type": pick("allow_apply_type"),
        "application_check_type": pick("application_check_type"),
        "is_need_check_application": pick("is_need_check_application"),
        "create_time": pick("create_time"),
        "edit_time": pick("edit_time"),
        "source_language": pick("source_language"),
        "target_languages": pick("target_languages"),
        // No server-side Output row exists for a local export, so this is a local marker
        // rather than an ObjectId.
        "output_id": format!("local-{}", request.target_id),
        "output_language": pick("target_languages"),
    })
}

fn write_zip(
    destination: &Path,
    translations: &str,
    project_json: serde_json::Value,
    images: &BTreeMap<String, PathBuf>,
    skipped: &[String],
) -> Result<(), String> {
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    let file = std::fs::File::create(destination).map_err(|err| format!("创建文件失败：{err}"))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    zip.start_file("translations.txt", options)
        .map_err(|err| err.to_string())?;
    zip.write_all(translations.as_bytes())
        .map_err(|err| err.to_string())?;

    if !skipped.is_empty() {
        // Same shape as the server's errors.txt.
        let mut report = format!("Project ID:   {}\n", "");
        report.push_str("------------------------\r\n");
        for name in skipped {
            report.push_str(&format!("File {name} download error.\r\n"));
        }
        zip.start_file("errors.txt", options)
            .map_err(|err| err.to_string())?;
        zip.write_all(report.as_bytes()).map_err(|err| err.to_string())?;
    }

    let json = serde_json::to_string(&project_json).map_err(|err| err.to_string())?;
    zip.start_file("project.json", options)
        .map_err(|err| err.to_string())?;
    zip.write_all(json.as_bytes()).map_err(|err| err.to_string())?;

    for (name, path) in images {
        zip.start_file(format!("images/{name}"), options)
            .map_err(|err| err.to_string())?;
        let bytes = std::fs::read(path).map_err(|err| format!("读取 {name} 失败：{err}"))?;
        zip.write_all(&bytes).map_err(|err| err.to_string())?;
    }

    zip.finish().map_err(|err| err.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_name_pads_numeric_runs_to_six_digits() {
        assert_eq!(sort_name("1.jpg"), "000001");
        assert_eq!(sort_name("10.jpg"), "000010");
        assert_eq!(sort_name("2.jpg"), "000002");
    }

    /// The padding is what makes "page2" sort before "page10".
    #[test]
    fn sort_name_orders_naturally() {
        assert!(sort_name("page2.jpg") < sort_name("page10.jpg"));
        assert!(sort_name("1.jpg") < sort_name("2.jpg"));
    }

    #[test]
    fn sort_name_splits_mixed_runs() {
        // "book1-002" -> "book" + "000001" + "-" + "000002"
        assert_eq!(sort_name("book1-002.jpg"), "book000001-000002");
    }

    #[test]
    fn sort_name_keeps_names_without_extension() {
        assert_eq!(sort_name("001"), "000001");
    }

    #[test]
    fn sort_name_truncates_oversized_numbers_without_padding() {
        assert_eq!(sort_name("1234567.jpg"), "1234567");
    }

    #[test]
    fn split_runs_matches_python_findall() {
        assert_eq!(split_digit_runs("book1-002"), vec!["book", "1", "-", "002"]);
        assert_eq!(split_digit_runs("abc"), vec!["abc"]);
        assert_eq!(split_digit_runs("123"), vec!["123"]);
    }

    fn source(rank: i64, mine: Option<ApiTranslation>, others: Vec<ApiTranslation>) -> ApiSource {
        ApiSource {
            x: 0.0,
            y: 0.0,
            rank,
            position_type: 1,
            my_translation: mine,
            translations: others,
        }
    }

    fn translation(content: &str, proofread: &str, selected: bool, edit: &str) -> ApiTranslation {
        ApiTranslation {
            content: content.into(),
            proofread_content: proofread.into(),
            selected,
            edit_time: edit.into(),
        }
    }

    #[test]
    fn selected_translation_wins() {
        let s = source(
            0,
            Some(translation("mine", "", false, "2026-01-02T00:00:00")),
            vec![translation("other", "", true, "2026-01-01T00:00:00")],
        );
        assert_eq!(best_content(&s), "other");
    }

    #[test]
    fn proofread_beats_newer_unproofread() {
        let s = source(
            0,
            Some(translation("newer", "", false, "2026-05-01T00:00:00")),
            vec![translation("older", "checked", false, "2026-01-01T00:00:00")],
        );
        assert_eq!(best_content(&s), "checked");
    }

    #[test]
    fn newest_wins_when_nothing_else_separates_them() {
        let s = source(
            0,
            Some(translation("old", "", false, "2020-01-01T00:00:00")),
            vec![translation("new", "", false, "2026-01-01T00:00:00")],
        );
        assert_eq!(best_content(&s), "new");
    }

    #[test]
    fn untranslated_source_yields_empty_string() {
        assert_eq!(best_content(&source(0, None, vec![])), "");
    }

    /// `proofread_content or content` — an empty proofread falls back to content.
    #[test]
    fn empty_proofread_falls_back_to_content() {
        let s = source(0, Some(translation("body", "", false, "2026-01-01T00:00:00")), vec![]);
        assert_eq!(best_content(&s), "body");
    }

    #[test]
    fn dense_ranks_are_not_flagged() {
        let s = vec![source(0, None, vec![]), source(1, None, vec![]), source(2, None, vec![])];
        assert!(!has_rank_gap(&s));
    }

    /// A hole in the rank sequence is how we detect that blank sources were filtered out.
    #[test]
    fn a_hole_in_ranks_is_detected() {
        let s = vec![source(0, None, vec![]), source(2, None, vec![])];
        assert!(has_rank_gap(&s));
    }

    #[test]
    fn empty_source_list_is_not_flagged() {
        assert!(!has_rank_gap(&[]));
    }

    #[test]
    fn ranks_starting_above_zero_are_not_flagged() {
        let s = vec![source(5, None, vec![]), source(6, None, vec![])];
        assert!(!has_rank_gap(&s));
    }
}
