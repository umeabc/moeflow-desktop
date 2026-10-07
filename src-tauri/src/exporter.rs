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

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

use crate::labelplus::{self, LpFile, LpLocale, LpSource};
use crate::media::MediaCache;
use crate::profiles::Profile;

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
    /// Signed storage URL used by the browser and the server's export task.
    #[serde(default)]
    url: String,
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
fn best_content(source: &ApiSource) -> String {
    let mut all: Vec<&ApiTranslation> = Vec::new();
    if let Some(mine) = source.my_translation.as_ref() {
        all.push(mine);
    }
    all.extend(source.translations.iter());

    // Re-sort after combining my_translation and translations: the former is not
    // necessarily the candidate the server would choose.
    all.sort_by(|a, b| {
        b.selected
            .cmp(&a.selected)
            .then_with(|| b.proofread_content.cmp(&a.proofread_content))
            .then_with(|| b.edit_time.cmp(&a.edit_time))
    });
    match all.first() {
        Some(row) if !row.proofread_content.is_empty() => row.proofread_content.clone(),
        Some(row) => row.content.clone(),
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
    client: &'a crate::network::NetworkClient,
    api_base: &'a str,
    token: &'a str,
}

impl<'a> Api<'a> {
    async fn get_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T, String> {
        let value = self.get_value(path).await?;
        serde_json::from_value(value).map_err(|err| format!("解析 {path} 响应失败：{err}"))
    }

    async fn get_value(&self, path: &str) -> Result<serde_json::Value, String> {
        self.get_response(path)
            .await?
            .json::<serde_json::Value>()
            .await
            .map_err(|err| format!("解析 {path} 响应失败：{err}"))
    }

    async fn get_response(&self, path: &str) -> Result<reqwest::Response, String> {
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
        Ok(response)
    }

    async fn list_files(&self, path: &str) -> Result<Vec<ApiFile>, String> {
        let mut files = Vec::new();
        let mut seen = HashSet::new();
        let separator = if path.contains('?') { '&' } else { '?' };
        // Flask-apikit uses one-based page/limit and X-Pagination-Count. Some
        // deployments omit that header; keep paging until an empty page there.
        for page in 1.. {
            let page_path = format!("{path}{separator}page={page}&limit=50");
            let response = self.get_response(&page_path).await?;
            let count = response
                .headers()
                .get("x-pagination-count")
                .map(|value| {
                    value
                        .to_str()
                        .ok()
                        .and_then(|text| text.parse::<usize>().ok())
                        .ok_or_else(|| format!("{page_path} 返回无效的分页总数"))
                })
                .transpose()?;
            let value: serde_json::Value = response
                .json()
                .await
                .map_err(|err| format!("解析 {page_path} 响应失败：{err}"))?;
            let rows = if value.is_array() {
                value
            } else {
                value
                    .get("data")
                    .or_else(|| value.get("files"))
                    .filter(|rows| rows.is_array())
                    .cloned()
                    .ok_or_else(|| format!("{page_path} 文件列表响应格式无法识别"))?
            };
            let entries: Vec<ApiFile> = serde_json::from_value(rows)
                .map_err(|err| format!("解析 {page_path} 响应失败：{err}"))?;
            if entries.is_empty() {
                if count.is_some_and(|count| files.len() < count) {
                    return Err(format!("{page_path} 文件列表提前结束，无法完整导出"));
                }
                break;
            }
            for entry in entries {
                if !seen.insert(entry.id.clone()) {
                    return Err(format!("{page_path} 返回重复文件，分页结果无法完整导出"));
                }
                files.push(entry);
            }
            if count.is_some_and(|count| files.len() >= count) {
                break;
            }
        }
        Ok(files)
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
    url: String,
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
        let entries = api.list_files(&path).await?;

        let mut children: Vec<Walked> = entries
            .into_iter()
            .map(|entry| Walked {
                id: entry.id,
                sort_name: sort_name(&entry.name),
                name: entry.name,
                dir: dir.clone(),
                dir_sort_name: dir_sort_name.clone(),
                file_type: entry.file_type,
                url: entry.url,
            })
            .collect();
        children.sort_by(|a, b| a.sort_name.cmp(&b.sort_name));

        let folders: Vec<&Walked> = children.iter().filter(|c| c.file_type == 1).collect();
        for folder in folders.iter().rev() {
            let mut child_dir = dir.clone();
            child_dir.push(folder.name.clone());
            let child_dir_sort = format!("{}{}/", folder.dir_sort_name, folder.sort_name);
            stack.push((Some(folder.id.clone()), child_dir, child_dir_sort));
        }

        out.extend(children);
    }

    Ok(out)
}

fn image_url(profile: &Profile, file: &Walked) -> Result<String, String> {
    if file.url.trim().is_empty() {
        return Ok(format!(
            "{}/v1/files/{}/content",
            profile.api_base.trim_end_matches('/'),
            file.id
        ));
    }
    // Relative storage paths belong to the site, not its split API host or /api prefix.
    let base = reqwest::Url::parse(&format!("{}/", profile.site_origin()))
        .map_err(|err| format!("站点地址无效：{err}"))?;
    let url = base
        .join(&file.url)
        .map_err(|err| format!("图片地址无效：{err}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("图片地址必须使用 HTTP 或 HTTPS".into());
    }
    Ok(url.into())
}

/// Fetch an image into the media cache (or reuse it) and return its on-disk path.
async fn image_path(
    client: &crate::network::NetworkClient,
    profile: &Profile,
    cache: &MediaCache,
    token: &str,
    url: &str,
) -> Result<PathBuf, String> {
    if let Some(path) = cache.get(url) {
        return Ok(path);
    }

    let mut request = client.get(url);
    if let Some(referer) = profile.media_referer() {
        request = request.header(reqwest::header::REFERER, referer);
    }
    // Signed storage URLs do not need the API token. Compare origins, not string
    // prefixes, so api.example.evil cannot receive credentials for api.example.
    let same_origin = reqwest::Url::parse(url)
        .ok()
        .zip(reqwest::Url::parse(&profile.api_base).ok())
        .is_some_and(|(image, api)| image.origin() == api.origin());
    if same_origin {
        request = request.header("Authorization", format!("Bearer {token}"));
    }

    let response = request
        .send()
        .await
        .map_err(|err| format!("请求图片失败：{err}"))?;
    if !response.status().is_success() {
        return Err(format!("图片请求返回 HTTP {}（{url}）", response.status()));
    }
    if let Some(content_type) = response.headers().get(reqwest::header::CONTENT_TYPE) {
        let content_type = content_type.to_str().unwrap_or("").to_ascii_lowercase();
        // A number of deployments route unknown paths to index.html with status 200. Never
        // cache that HTML shell under an image URL — it makes the failure persistent.
        if content_type.starts_with("text/html")
            || content_type.starts_with("application/xhtml+xml")
            || content_type.starts_with("application/json")
        {
            return Err(format!("图片接口返回了 {content_type} 而不是图片（{url}）"));
        }
    }

    let temp = cache.temp_path();
    let result = async {
        let mut file = tokio::fs::File::create(&temp)
            .await
            .map_err(|err| err.to_string())?;
        let mut stream = response.bytes_stream();
        let mut prefix = Vec::new();
        let mut size = 0usize;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|err| err.to_string())?;
            prefix.extend_from_slice(&chunk[..chunk.len().min(128 - prefix.len())]);
            size += chunk.len();
            tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
                .await
                .map_err(|err| err.to_string())?;
        }
        drop(file);
        if size == 0 {
            return Err("图片接口返回了空文件".into());
        }
        // HTML fallbacks are sometimes mislabeled as application/octet-stream.
        let text = String::from_utf8_lossy(&prefix)
            .trim_start_matches('\u{feff}')
            .trim_start()
            .to_ascii_lowercase();
        if ["<!doctype html", "<html", "<head", "<body"]
            .iter()
            .any(|tag| text.starts_with(tag))
        {
            return Err("图片接口返回了 HTML 而不是图片".into());
        }
        cache
            .commit(url, &temp, None)
            .map_err(|err| err.to_string())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temp).await;
    }
    result
}

/// Run a full local export.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    client: &crate::network::NetworkClient,
    profile: &Profile,
    cache: Arc<MediaCache>,
    request: &ExportRequest,
    mut progress: impl FnMut(&str, f32),
) -> Result<ExportReport, String> {
    let api = Api {
        client,
        api_base: &profile.api_base,
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

    let images: Vec<&Walked> = tree
        .iter()
        .filter(|f| f.file_type == FILE_TYPE_IMAGE)
        .collect();
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
            // Official MoeFlow supplies File.url; /content is an optional fork
            // extension, not a route that the supported deployments guarantee.
            let url = image_url(profile, file);
            let result = match url {
                Ok(url) => image_path(client, profile, &cache, &request.token, &url).await,
                Err(err) => Err(err),
            };
            match result {
                Ok(path) => {
                    image_files.insert(file.name.clone(), path);
                }
                Err(err) => {
                    skipped_images.push(format!("{}（{err}）", file.name));
                }
            }
        }
    }

    if request.include_images && !images.is_empty() && image_files.is_empty() {
        return Err(format!(
            "全部 {} 张图片下载失败，未生成导出文件：{}",
            images.len(),
            skipped_images.join("；")
        ));
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
        .get_value(&format!("v1/projects/{}", request.project_id))
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
        zip.write_all(report.as_bytes())
            .map_err(|err| err.to_string())?;
    }

    let json = serde_json::to_string(&project_json).map_err(|err| err.to_string())?;
    zip.start_file("project.json", options)
        .map_err(|err| err.to_string())?;
    zip.write_all(json.as_bytes())
        .map_err(|err| err.to_string())?;

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
            vec![translation(
                "older",
                "checked",
                false,
                "2026-01-01T00:00:00",
            )],
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
        let s = source(
            0,
            Some(translation("body", "", false, "2026-01-01T00:00:00")),
            vec![],
        );
        assert_eq!(best_content(&s), "body");
    }

    #[test]
    fn dense_ranks_are_not_flagged() {
        let s = vec![
            source(0, None, vec![]),
            source(1, None, vec![]),
            source(2, None, vec![]),
        ];
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

    #[test]
    fn selected_unproofread_beats_unselected_proofread() {
        let s = source(
            0,
            Some(translation("selected", "", true, "2020")),
            vec![translation("other", "checked", false, "2026")],
        );
        assert_eq!(best_content(&s), "selected");
    }

    #[test]
    fn proofread_sort_is_lexicographic_not_just_presence() {
        let s = source(
            0,
            Some(translation("mine", "alpha", false, "2026")),
            vec![translation("other", "zulu", false, "2020")],
        );
        assert_eq!(best_content(&s), "zulu");
    }

    #[test]
    fn newest_mine_beats_last_candidate() {
        let s = source(
            0,
            Some(translation("new", "same", false, "2026")),
            vec![translation("old", "same", false, "2020")],
        );
        assert_eq!(best_content(&s), "same");
        let s = source(
            0,
            Some(translation("new", "", false, "2026")),
            vec![translation("old", "", false, "2020")],
        );
        assert_eq!(best_content(&s), "new");
    }

    #[derive(Clone, Copy)]
    enum MockMode {
        Normal,
        NoCount,
        Html,
        MislabeledHtml,
        EmptyImage,
        Missing,
        Partial,
        UnknownList,
        RepeatPage,
        ContentFallback,
        BlankGap,
    }

    #[derive(Clone)]
    struct MockState {
        mode: MockMode,
        site: String,
        calls: Arc<std::sync::Mutex<Vec<(String, Option<String>, Option<String>)>>>,
    }

    // A valid, byte-exact 1x1 PNG, not a URL placeholder or an HTML response.
    fn png_bytes() -> Vec<u8> {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.decode(
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="
        ).unwrap()
    }

    async fn mock_http(
        axum::extract::State(state): axum::extract::State<MockState>,
        uri: axum::http::Uri,
        headers: axum::http::HeaderMap,
    ) -> axum::response::Response {
        use axum::http::StatusCode;
        use axum::response::IntoResponse;
        let header = |name| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        state.calls.lock().unwrap().push((
            uri.to_string(),
            header("referer"),
            header("authorization"),
        ));
        let path = uri.path();
        let query: BTreeMap<String, String> = reqwest::Url::parse(&format!("http://mock{uri}"))
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect();
        if path == "/api/v1/projects/project/files" {
            assert_eq!(
                header("authorization").as_deref(),
                Some("Bearer test-token")
            );
            assert_eq!(query.get("limit").map(String::as_str), Some("50"));
            if matches!(state.mode, MockMode::UnknownList) {
                return axum::Json(serde_json::json!({"unexpected": []})).into_response();
            }
            let page = query.get("page").unwrap().parse::<usize>().unwrap();
            let folder = query.get("parent_id").map(String::as_str);
            // Deliberately cap server pages at two rows, regardless of requested limit.
            let rows = match folder {
                None if page == 1 || matches!(state.mode, MockMode::RepeatPage) => {
                    serde_json::json!([
                        {"id":"f10", "name":"10.png", "type":2, "url":format!("{}/storage/10.png?signature=keep-me", state.site)},
                        {"id":"folder", "name":"chapter2", "type":1}
                    ])
                }
                None if page == 2 => serde_json::json!([
                    {"id":"f2", "name":"2.png", "type":2, "url":if matches!(state.mode, MockMode::ContentFallback) { "" } else { "/storage/2.png" }}
                ]),
                Some("folder") if page == 1 => serde_json::json!([
                    {"id":"nested", "name":"1.png", "type":2, "url":"/storage/nested.png"}
                ]),
                _ => serde_json::json!([]),
            };
            let mut response = axum::Json(rows).into_response();
            if !matches!(state.mode, MockMode::NoCount | MockMode::RepeatPage) {
                response.headers_mut().insert(
                    "x-pagination-count",
                    if folder.is_some() { "1" } else { "3" }.parse().unwrap(),
                );
            }
            return response;
        }
        if path.starts_with("/api/v1/files/") && path.ends_with("/sources") {
            assert_eq!(query.get("target_id").map(String::as_str), Some("target"));
            assert_eq!(query.get("paging").map(String::as_str), Some("false"));
            assert_eq!(query.get("show_blank").map(String::as_str), Some("true"));
            let mut rows = serde_json::json!([
                {"x":0.1,"y":0.2,"rank":0,"position_type":1,
                    "my_translation":{"content":"selected","selected":true,"edit_time":"2020"},
                    "translations":[{"content":"wrong","proofread_content":"checked","edit_time":"2026"}]},
                {"x":0.3,"y":0.4,"rank":1,"position_type":2,
                    "my_translation":{"content":"wrong","proofread_content":"alpha","edit_time":"2026"},
                    "translations":[{"content":"other","proofread_content":"zulu","edit_time":"2020"}]},
                {"x":0.5,"y":0.6,"rank":2,"position_type":1,
                    "my_translation":{"content":"newest","edit_time":"2026"},
                    "translations":[{"content":"wrong","edit_time":"2020"}]},
                {"x":0.7,"y":0.8,"rank":3,"position_type":1,"content":"must remain blank"}
            ]);
            if matches!(state.mode, MockMode::BlankGap) {
                rows.as_array_mut().unwrap().remove(1);
            }
            return axum::Json(rows).into_response();
        }
        if path == "/api/v1/projects/project" {
            return axum::Json(serde_json::json!({"name":"mock project"})).into_response();
        }
        if path.starts_with("/storage/") || path == "/api/v1/files/f2/content" {
            assert_eq!(header("referer"), Some(format!("{}/", state.site)));
            if path.starts_with("/storage/") {
                // The signed storage host is distinct from the API host. Do not leak tokens.
                assert_eq!(header("authorization"), None);
            } else {
                assert_eq!(
                    header("authorization").as_deref(),
                    Some("Bearer test-token")
                );
            }
            if matches!(state.mode, MockMode::Html)
                || (matches!(state.mode, MockMode::Partial) && path == "/storage/2.png")
            {
                return (
                    [("content-type", "text/html; charset=utf-8")],
                    "<html>login shell</html>",
                )
                    .into_response();
            }
            if matches!(state.mode, MockMode::MislabeledHtml) {
                return (
                    [("content-type", "application/octet-stream")],
                    "  <!DOCTYPE html><html>shell</html>",
                )
                    .into_response();
            }
            if matches!(state.mode, MockMode::EmptyImage) {
                return ([("content-type", "image/png")], Vec::<u8>::new()).into_response();
            }
            if matches!(state.mode, MockMode::Missing) {
                return StatusCode::NOT_FOUND.into_response();
            }
            return ([("content-type", "image/png")], png_bytes()).into_response();
        }
        // Official deployments do NOT provide /v1/files/:id/content.
        StatusCode::NOT_FOUND.into_response()
    }

    struct MockExport {
        profile: Profile,
        request: ExportRequest,
        cache: Arc<MediaCache>,
        state: MockState,
        servers: Vec<tokio::task::JoinHandle<()>>,
        dir: PathBuf,
    }

    impl MockExport {
        async fn new(mode: MockMode) -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static SEQ: AtomicUsize = AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "moeflow-export-test-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let media = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let api = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let site = format!("http://{}", media.local_addr().unwrap());
            let state = MockState {
                mode,
                site: site.clone(),
                calls: Arc::new(std::sync::Mutex::new(Vec::new())),
            };
            let profile = Profile {
                id: "test".into(),
                name: "test".into(),
                site_url: format!("{site}/workbench"),
                api_base: format!("http://{}/api", api.local_addr().unwrap()),
                port: 0,
                allow_invalid_certs: false,
                media_origins: vec![],
            };
            let request = ExportRequest {
                project_id: "project".into(),
                target_id: "target".into(),
                token: "test-token".into(),
                locale: "en".into(),
                destination: dir.join("export.zip"),
                include_images: true,
            };
            let cache = Arc::new(MediaCache::new(dir.join("cache"), 1024 * 1024));
            let mut servers = Vec::new();
            for listener in [api, media] {
                let app = axum::Router::new()
                    .fallback(mock_http)
                    .with_state(state.clone());
                servers.push(tokio::spawn(async move {
                    axum::serve(listener, app).await.unwrap();
                }));
            }
            Self {
                profile,
                request,
                cache,
                state,
                servers,
                dir,
            }
        }

        async fn export(&self) -> Result<ExportReport, String> {
            run(
                &crate::network::NetworkClient::build(
                    &crate::network::ProxySettings::default(),
                    false,
                    false,
                )
                .unwrap(),
                &self.profile,
                self.cache.clone(),
                &self.request,
                |_, _| {},
            )
            .await
        }

        fn zip_bytes(&self, name: &str) -> Vec<u8> {
            use std::io::Read;
            let mut archive =
                zip::ZipArchive::new(std::fs::File::open(&self.request.destination).unwrap())
                    .unwrap();
            let mut bytes = Vec::new();
            archive
                .by_name(name)
                .unwrap()
                .read_to_end(&mut bytes)
                .unwrap();
            bytes
        }
    }

    impl Drop for MockExport {
        fn drop(&mut self) {
            for server in &self.servers {
                server.abort();
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[tokio::test]
    async fn export_uses_file_urls_referer_pagination_and_byte_exact_legacy_rules() {
        let fixture = MockExport::new(MockMode::Normal).await;
        let report = fixture.export().await.unwrap();
        assert_eq!(report.file_count, 3);
        assert_eq!(report.image_count, 3);
        assert!(!report.diverged);
        assert!(report.skipped_images.is_empty());
        for name in ["2.png", "10.png", "1.png"] {
            assert_eq!(fixture.zip_bytes(&format!("images/{name}")), png_bytes());
        }
        let expected = [
            (Vec::new(), "2.png"),
            (Vec::new(), "10.png"),
            (vec!["chapter2".into()], "1.png"),
        ]
        .into_iter()
        .map(|(dir, name)| LpFile {
            dir,
            name: name.into(),
            sources: vec![
                LpSource {
                    x: 0.1,
                    y: 0.2,
                    position_type: 1,
                    content: "selected".into(),
                },
                LpSource {
                    x: 0.3,
                    y: 0.4,
                    position_type: 2,
                    content: "zulu".into(),
                },
                LpSource {
                    x: 0.5,
                    y: 0.6,
                    position_type: 1,
                    content: "newest".into(),
                },
                LpSource {
                    x: 0.7,
                    y: 0.8,
                    position_type: 1,
                    content: String::new(),
                },
            ],
        })
        .collect::<Vec<_>>();
        assert_eq!(
            fixture.zip_bytes("translations.txt"),
            labelplus::render_document(&expected, LpLocale::from_tag("en")).as_bytes()
        );
        let calls = fixture.state.calls.lock().unwrap();
        let routes: Vec<_> = calls.iter().map(|(path, _, _)| path.as_str()).collect();
        assert_eq!(
            routes
                .iter()
                .filter(|route| route.starts_with("/api/v1/projects/project/files"))
                .count(),
            3
        );
        assert!(routes.contains(&"/api/v1/projects/project/files?page=2&limit=50"));
        assert!(routes.contains(&"/api/v1/projects/project/files?parent_id=folder&page=1&limit=50"));
        assert!(routes.contains(&"/storage/10.png?signature=keep-me"));
        assert!(!routes
            .iter()
            .any(|route| route.ends_with("/content") || route.starts_with("/api/projects/")));
    }

    #[tokio::test]
    async fn export_pages_until_empty_when_count_header_is_missing() {
        let fixture = MockExport::new(MockMode::NoCount).await;
        assert_eq!(fixture.export().await.unwrap().image_count, 3);
        let calls = fixture.state.calls.lock().unwrap();
        assert!(calls
            .iter()
            .any(|(path, _, _)| path == "/api/v1/projects/project/files?page=3&limit=50"));
        assert!(calls.iter().any(|(path, _, _)| path
            == "/api/v1/projects/project/files?parent_id=folder&page=2&limit=50"));
    }

    #[tokio::test]
    async fn all_image_failures_and_html_are_errors_not_successful_empty_archives() {
        for mode in [MockMode::Html, MockMode::Missing] {
            let fixture = MockExport::new(mode).await;
            let err = fixture.export().await.unwrap_err();
            assert!(err.contains("全部 3 张图片下载失败"), "{err}");
            assert!(!fixture.request.destination.exists());
            assert_eq!(fixture.cache.stats().entries, 0);
            if matches!(mode, MockMode::Html) {
                assert!(err.contains("text/html"));
            } else {
                assert!(err.contains("404"));
            }
        }
    }

    #[tokio::test]
    async fn mislabeled_html_and_empty_image_are_not_cached() {
        for mode in [MockMode::MislabeledHtml, MockMode::EmptyImage] {
            let fixture = MockExport::new(mode).await;
            assert!(fixture
                .export()
                .await
                .unwrap_err()
                .contains("全部 3 张图片下载失败"));
            assert_eq!(fixture.cache.stats().entries, 0);
            assert!(!fixture.request.destination.exists());
            assert_eq!(std::fs::read_dir(fixture.cache.dir()).unwrap().count(), 0);
        }
    }

    #[tokio::test]
    async fn a_second_export_reuses_cached_bytes_without_media_requests() {
        let fixture = MockExport::new(MockMode::Normal).await;
        fixture.export().await.unwrap();
        fixture.state.calls.lock().unwrap().clear();
        assert_eq!(fixture.export().await.unwrap().image_count, 3);
        assert_eq!(fixture.zip_bytes("images/2.png"), png_bytes());
        assert!(!fixture
            .state
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|(path, _, _)| path.starts_with("/storage/")));
    }

    #[tokio::test]
    async fn partial_image_failure_keeps_valid_images_and_errors_report() {
        let fixture = MockExport::new(MockMode::Partial).await;
        let report = fixture.export().await.unwrap();
        assert_eq!(report.image_count, 2);
        assert_eq!(report.skipped_images.len(), 1);
        assert_eq!(fixture.cache.stats().entries, 2);
        assert_eq!(fixture.zip_bytes("images/10.png"), png_bytes());
        assert!(String::from_utf8(fixture.zip_bytes("errors.txt"))
            .unwrap()
            .contains("text/html"));
    }

    #[tokio::test]
    async fn unknown_file_shapes_and_repeated_pages_are_explicit_errors() {
        for mode in [MockMode::UnknownList, MockMode::RepeatPage] {
            let fixture = MockExport::new(mode).await;
            assert!(fixture.export().await.is_err());
            assert!(!fixture.request.destination.exists());
        }
    }

    #[tokio::test]
    async fn content_extension_is_only_used_when_storage_url_is_absent() {
        let fixture = MockExport::new(MockMode::ContentFallback).await;
        assert_eq!(fixture.export().await.unwrap().image_count, 3);
        assert_eq!(fixture.zip_bytes("images/2.png"), png_bytes());
        let calls = fixture.state.calls.lock().unwrap();
        assert_eq!(
            calls
                .iter()
                .filter(|(path, _, _)| path.ends_with("/content"))
                .count(),
            1
        );
        assert!(calls
            .iter()
            .any(|(path, _, _)| path == "/api/v1/files/f2/content"));
    }

    #[tokio::test]
    async fn txt_only_export_does_not_download_images() {
        let mut fixture = MockExport::new(MockMode::Missing).await;
        fixture.request.include_images = false;
        let report = fixture.export().await.unwrap();
        assert_eq!(report.file_count, 3);
        assert_eq!(report.image_count, 0);
        assert!(report.skipped_images.is_empty());
        assert!(!fixture
            .state
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|(path, _, _)| path.starts_with("/storage/") || path.ends_with("/content")));
    }

    #[tokio::test]
    async fn hidden_blank_sources_still_warn_about_label_divergence() {
        let fixture = MockExport::new(MockMode::BlankGap).await;
        let report = fixture.export().await.unwrap();
        assert!(report.diverged);
        assert_eq!(report.warnings.len(), 1);
    }
}
