//! GitHub Releases storage and the streaming part writer.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::core::errors::CoreError;
use crate::core::events::{self, EventProgress};
use crate::core::fmt;
use crate::core::manifest;
use crate::core::paths;
use crate::core::text;

pub const CHUNK_SIZE: u64 = 200 * 1024 * 1024;
const API: &str = "https://api.github.com";
const UPLOAD: &str = "https://uploads.github.com";
const MAX_RETRIES: u32 = 8;

/// True when `value` is exactly two non-empty `[A-Za-z0-9_.-]` segments with no `..`.
pub fn valid_repo(value: &str) -> bool {
    let segments: Vec<&str> = value.split('/').collect();
    segments.len() == 2
        && segments.iter().all(|segment| {
            !segment.is_empty()
                && !segment.contains("..")
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        })
}

/// Percent-encode everything outside `A-Z a-z 0-9 - . _ ~` with uppercase hex.
pub fn encode_segment(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        let keep = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~');
        if keep {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

/// Parse a release-list response into a vector, or error on a non-2xx status.
pub fn releases_from_response(status: u16, body: &str) -> Result<Vec<Value>, CoreError> {
    if !(200..300).contains(&status) {
        return Err(CoreError::plain(format!(
            "GitHub API error {status}: {body}"
        )));
    }
    let value: Value = serde_json::from_str(body).map_err(CoreError::runtime)?;
    value
        .as_array()
        .cloned()
        .ok_or_else(|| CoreError::runtime("GitHub releases response was not an array"))
}

/// True when the remote asset reports exactly `local_size` bytes.
pub fn asset_matches(remote: &Value, local_size: u64) -> bool {
    remote.get("size").and_then(Value::as_u64) == Some(local_size)
}

/// Group `.partNNNN` files in `dest` by their base name, or error when `dest` is unreadable.
pub fn part_groups(dest: &Path) -> Result<HashMap<String, Vec<PathBuf>>, CoreError> {
    let mut groups: HashMap<String, Vec<PathBuf>> = HashMap::new();
    let entries = fs::read_dir(dest).map_err(CoreError::os)?;
    static PART_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(.+)\.part(\d{4})$").expect("part-name regex is a valid literal")
    });
    let part_re = &*PART_RE;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(captures) = part_re.captures(&name)
            && let Some(base) = captures.get(1)
        {
            groups
                .entry(base.as_str().to_string())
                .or_default()
                .push(entry.path());
        }
    }
    Ok(groups)
}

#[derive(Clone)]
struct StreamState {
    release_id: i64,
    work_dir: PathBuf,
}

#[derive(Clone)]
pub struct GitHubStorage {
    token: String,
    repo: String,
    branch: String,
    agent: ureq::Agent,
    stream: Option<Arc<StreamState>>,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(true)
        .http_status_as_error(false)
        .max_redirects(10)
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .timeout_per_read(Some(Duration::from_secs(120)))
        .timeout_per_write(Some(Duration::from_secs(120)))
        .user_agent("termux-sync/1.3.9")
        .build()
        .into()
}

impl GitHubStorage {
    pub fn new(cfg: &Value) -> Result<Self, CoreError> {
        let token = cfg
            .get("github_token")
            .map(text::py_str)
            .unwrap_or_default();
        let repo = cfg.get("github_repo").map(text::py_str).unwrap_or_default();
        let branch = cfg
            .get("github_branch")
            .map(text::py_str)
            .filter(|branch| !branch.is_empty())
            .unwrap_or_else(|| "main".to_string());
        if token.is_empty() || repo.is_empty() {
            return Err(CoreError::user(
                "GitHub is not configured: the token or repository is not set.",
            ));
        }
        if !valid_repo(&repo) {
            return Err(CoreError::user("Invalid GitHub repository; use owner/repo"));
        }
        Ok(Self {
            token,
            repo,
            branch,
            agent: agent(),
            stream: None,
        })
    }

    fn auth(&self) -> String {
        format!("token {}", self.token)
    }

    fn request_raw(
        &self,
        method: &str,
        url: &str,
        body: Option<Value>,
    ) -> Result<ureq::http::Response<ureq::Body>, CoreError> {
        let response = if let Some(value) = body {
            let request = match method {
                "PUT" => self.agent.put(url),
                _ => self.agent.post(url),
            };
            let text = serde_json::to_string(&value).map_err(CoreError::value)?;
            request
                .header("Authorization", &self.auth())
                .header("Accept", "application/vnd.github.v3+json")
                .header("Content-Type", "application/json")
                .send(text)
                .map_err(CoreError::runtime)?
        } else {
            let request = match method {
                "DELETE" => self.agent.delete(url),
                _ => self.agent.get(url),
            };
            request
                .header("Authorization", &self.auth())
                .header("Accept", "application/vnd.github.v3+json")
                .call()
                .map_err(CoreError::runtime)?
        };
        Ok(response)
    }

    fn api(&self, method: &str, endpoint: &str, body: Option<Value>) -> Result<Value, CoreError> {
        let response = self.request_raw(method, &format!("{API}{endpoint}"), body)?;
        let status = response.status().as_u16();
        let text = response
            .into_body()
            .read_to_string()
            .map_err(CoreError::runtime)?;
        if !(200..300).contains(&status) {
            return Err(self.http_error(status, &text));
        }
        if text.trim().is_empty() {
            return Ok(json!({}));
        }
        serde_json::from_str(&text).map_err(CoreError::runtime)
    }

    fn http_error(&self, code: u16, raw: &str) -> CoreError {
        let parsed: Option<Value> = serde_json::from_str(raw).ok();
        let msg = parsed
            .as_ref()
            .and_then(|value| value.get("message"))
            .map(text::py_str)
            .unwrap_or_else(|| raw.trim().chars().take(300).collect());
        let extras = parsed
            .as_ref()
            .and_then(|value| value.get("errors"))
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("message"))
                    .map(text::py_str)
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .unwrap_or_default();
        let blob = format!("{msg} {extras}").to_lowercase();
        if matches!(code, 409 | 422) && blob.contains("empty") {
            return CoreError::user(format!(
                "GitHub repository is empty: The repository '{}' has no commits yet, and GitHub cannot create a release in an empty repository.",
                self.repo
            ));
        }
        match code {
            401 => CoreError::user(
                "Invalid GitHub token: GitHub rejected the token (bad credentials).",
            ),
            403 if blob.contains("rate limit") => CoreError::user(
                "GitHub rate limit reached: GitHub is limiting requests from this token for now.",
            ),
            403 => CoreError::user(format!(
                "GitHub denied access: {}",
                if msg.is_empty() {
                    "The token is not allowed to do this."
                } else {
                    &msg
                }
            )),
            404 => CoreError::user(format!(
                "GitHub repository not found: Repository '{}' was not found, or the token cannot see it.",
                self.repo
            )),
            _ => {
                let detail = [msg.as_str(), extras.as_str()]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(": ");
                CoreError::runtime(format!(
                    "GitHub API error {code}: {}",
                    if detail.is_empty() {
                        "no details"
                    } else {
                        &detail
                    }
                ))
            }
        }
    }

    pub fn preflight(&self) -> Result<(), CoreError> {
        self.api("GET", &format!("/repos/{}", self.repo), None)?;
        self.api(
            "GET",
            &format!("/repos/{}/commits?per_page=1", self.repo),
            None,
        )?;
        Ok(())
    }

    fn create_release(&self, backup_name: &str) -> Result<Value, CoreError> {
        self.api(
            "POST",
            &format!("/repos/{}/releases", self.repo),
            Some(json!({
                "tag_name": backup_name,
                "name": backup_name,
                "body": format!("termux-sync backup — {backup_name}"),
                "draft": false,
                "prerelease": false,
                "target_commitish": self.branch,
            })),
        )
    }

    fn get_release_by_tag(&self, tag: &str) -> Result<Option<Value>, CoreError> {
        let response = self.request_raw(
            "GET",
            &format!(
                "{API}/repos/{}/releases/tags/{}",
                self.repo,
                encode_segment(tag)
            ),
            None,
        )?;
        let status = response.status().as_u16();
        let text = response
            .into_body()
            .read_to_string()
            .map_err(CoreError::runtime)?;
        if status == 404 {
            return Ok(None);
        }
        if !(200..300).contains(&status) {
            return Err(self.http_error(status, &text));
        }
        serde_json::from_str(&text)
            .map(Some)
            .map_err(CoreError::runtime)
    }

    fn list_releases(&self) -> Result<Vec<Value>, CoreError> {
        let response = self.request_raw(
            "GET",
            &format!("{API}/repos/{}/releases?per_page=100", self.repo),
            None,
        )?;
        let status = response.status().as_u16();
        let body = response
            .into_body()
            .read_to_string()
            .map_err(CoreError::runtime)?;
        releases_from_response(status, &body)
    }

    fn release_for(&self, backup_name: &str) -> Result<Value, CoreError> {
        if let Some(release) = self.get_release_by_tag(backup_name)? {
            return Ok(release);
        }
        self.create_release(backup_name)
    }

    fn split_file(&self, src: &Path, tmp_dir: &Path) -> Result<Vec<PathBuf>, CoreError> {
        self.split_file_at(src, tmp_dir, CHUNK_SIZE)
    }

    fn split_file_at(
        &self,
        src: &Path,
        tmp_dir: &Path,
        chunk: u64,
    ) -> Result<Vec<PathBuf>, CoreError> {
        let size = fs::metadata(src).map_err(CoreError::os)?.len();
        if chunk == 0 {
            return Err(CoreError::value("chunk size must be positive"));
        }
        if size <= chunk {
            return Ok(vec![src.to_path_buf()]);
        }
        let free = fs4::available_space(tmp_dir).map_err(CoreError::os)?;
        if free < size {
            return Err(CoreError::user(format!(
                "Not enough free space: Only {} is free, but {} is needed.",
                fmt::human_size(free as i64),
                fmt::human_size(size as i64)
            )));
        }
        let name = src
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut file = File::open(src).map_err(CoreError::os)?;
        let mut chunks = Vec::new();
        let mut index = 0u32;
        loop {
            let part_path = tmp_dir.join(format!("{name}.part{index:04}"));
            let output = File::create(&part_path).map_err(CoreError::os)?;
            let mut writer = BufWriter::with_capacity(8 * 1024, output);
            let copied =
                std::io::copy(&mut (&mut file).take(chunk), &mut writer).map_err(CoreError::os)?;
            writer.flush().map_err(CoreError::os)?;
            drop(writer);
            if copied == 0 {
                let _ = fs::remove_file(&part_path);
                break;
            }
            chunks.push(part_path);
            index += 1;
            if copied < chunk {
                break;
            }
        }
        Ok(chunks)
    }

    pub fn upload_asset(&self, release_id: i64, path: &Path) -> Result<(), CoreError> {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let size = fs::metadata(path).map_err(CoreError::os)?.len();
        let url = format!("{UPLOAD}/repos/{}/releases/{release_id}/assets", self.repo);
        let mut last_error = String::new();
        for attempt in 0..MAX_RETRIES {
            let file = File::open(path).map_err(CoreError::os)?;
            let mut reader = BufReader::with_capacity(128 * 1024, file).take(size);
            let response = self
                .agent
                .post(&url)
                .query("name", &name)
                .header("Authorization", &self.auth())
                .header("Accept", "application/vnd.github.v3+json")
                .header("Content-Type", "application/octet-stream")
                .header("Content-Length", &size.to_string())
                .config()
                .timeout_send_body(Some(Duration::from_secs(6 * 3600)))
                .build()
                .send(ureq::SendBody::from_reader(&mut reader));
            let response = match response {
                Ok(response) => response,
                Err(error) => {
                    last_error = format!("{error}");
                    std::thread::sleep(Duration::from_secs(15 * (attempt as u64 + 1)));
                    continue;
                }
            };
            let status = response.status().as_u16();
            let body = response.into_body().read_to_string().unwrap_or_default();
            if status == 201 {
                events::emit(
                    "progress",
                    vec![
                        ("stage", Value::String("upload".to_string())),
                        ("current", Value::from(size)),
                        ("total", Value::from(size)),
                        ("unit", Value::String("bytes".to_string())),
                        ("detail", Value::String(name)),
                    ],
                );
                return Ok(());
            }
            if status == 422 && body.contains("already_exists") {
                if let Some(asset) = self.release_assets(release_id)?.into_iter().find(|asset| {
                    asset.get("name").map(text::py_str).as_deref() == Some(name.as_str())
                }) {
                    if asset_matches(&asset, size) {
                        return Ok(());
                    }
                    if let Some(id) = asset.get("id").and_then(Value::as_i64) {
                        self.api(
                            "DELETE",
                            &format!("/repos/{}/releases/assets/{id}", self.repo),
                            None,
                        )?;
                    }
                    continue;
                }
                return Err(self.http_error(status, &body));
            }
            if status >= 500 || status == 429 {
                last_error = body;
                std::thread::sleep(Duration::from_secs(15 * (attempt as u64 + 1)));
                continue;
            }
            return Err(self.http_error(status, &body));
        }
        Err(CoreError::runtime(format!(
            "Failed to upload {name} after {MAX_RETRIES} attempts: {last_error}"
        )))
    }

    fn release_assets(&self, release_id: i64) -> Result<Vec<Value>, CoreError> {
        let response = self.request_raw(
            "GET",
            &format!(
                "{API}/repos/{}/releases/{release_id}/assets?per_page=100",
                self.repo
            ),
            None,
        )?;
        let status = response.status().as_u16();
        let body = response
            .into_body()
            .read_to_string()
            .map_err(CoreError::runtime)?;
        releases_from_response(status, &body)
    }

    pub fn upload(&self, backup_dir: &Path, backup_name: &str) -> Result<String, CoreError> {
        let release = self.release_for(backup_name)?;
        let release_id = release
            .get("id")
            .and_then(Value::as_i64)
            .ok_or_else(|| CoreError::runtime("GitHub release has no id"))?;
        let html_url = release
            .get("html_url")
            .map(text::py_str)
            .unwrap_or_default();
        let existing: Vec<String> = release
            .get("assets")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("name"))
                    .map(text::py_str)
                    .collect()
            })
            .unwrap_or_default();
        let split_dir = paths::tmp_root().join(format!("termux-sync-split-{backup_name}"));
        fs::create_dir_all(&split_dir).map_err(CoreError::os)?;
        let mut files: Vec<PathBuf> = fs::read_dir(backup_dir)
            .map_err(CoreError::os)?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .collect();
        files.sort();
        let result = (|| {
            for file in files {
                for chunk in self.split_file(&file, &split_dir)? {
                    let name = chunk
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    if existing.contains(&name) {
                        continue;
                    }
                    self.upload_asset(release_id, &chunk)?;
                }
            }
            Ok::<(), CoreError>(())
        })();
        let _ = fs::remove_dir_all(&split_dir);
        result?;
        Ok(html_url)
    }

    pub fn begin_stream(&mut self, backup_name: &str, work_dir: &Path) -> Result<(), CoreError> {
        fs::create_dir_all(work_dir).map_err(CoreError::os)?;
        let need = (CHUNK_SIZE as f64 * 1.2) as u64;
        let free = fs4::available_space(work_dir).map_err(CoreError::os)?;
        if free < need {
            return Err(CoreError::user(format!(
                "Not enough free space: Only {} is free, but about {} is needed to hold one upload part.",
                fmt::human_size(free as i64),
                fmt::human_size(need as i64)
            )));
        }
        let release = self.release_for(backup_name)?;
        let release_id = release
            .get("id")
            .and_then(Value::as_i64)
            .ok_or_else(|| CoreError::runtime("GitHub release has no id"))?;
        self.stream = Some(Arc::new(StreamState {
            release_id,
            work_dir: work_dir.to_path_buf(),
        }));
        Ok(())
    }

    pub fn stream_sink(&self, base_name: &str) -> Result<PartWriter, CoreError> {
        let stream = self
            .stream
            .as_ref()
            .ok_or_else(|| CoreError::runtime("GitHub stream was not started"))?;
        Ok(PartWriter::new(
            self.clone(),
            stream.release_id,
            stream.work_dir.clone(),
            base_name.to_string(),
        ))
    }

    pub fn abort_stream(&self, backup_name: &str) {
        let _ = self.delete_backup(backup_name);
    }

    pub fn fetch_manifest(&self, backup_name: &str) -> Option<Value> {
        let name = manifest::clean_backup_name(backup_name);
        self.list_backups().into_iter().find(|manifest| {
            manifest.get("name").map(text::py_str).as_deref() == Some(name.as_str())
        })
    }

    pub fn list_backups(&self) -> Vec<Value> {
        let Ok(releases) = self.list_releases() else {
            return Vec::new();
        };
        let mut backups = Vec::new();
        for release in releases {
            let tag = release
                .get("tag_name")
                .map(text::py_str)
                .unwrap_or_default();
            if !tag.starts_with("termux_backup_") {
                continue;
            }
            let mut manifest_value = None;
            if let Some(assets) = release.get("assets").and_then(Value::as_array) {
                for asset in assets {
                    if asset.get("name").map(text::py_str).as_deref() == Some("manifest.json") {
                        if let Some(id) = asset.get("id").and_then(Value::as_i64)
                            && let Ok(value) = self.download_asset_value(id)
                        {
                            manifest_value = Some(value);
                        }
                        break;
                    }
                }
            }
            backups.push(manifest_value.unwrap_or_else(|| {
                json!({
                    "name": tag,
                    "date": release.get("created_at").map(text::py_str).unwrap_or_default(),
                    "label": "",
                })
            }));
        }
        backups
    }

    fn download_asset_value(&self, asset_id: i64) -> Result<Value, CoreError> {
        let response = self
            .agent
            .get(&format!(
                "{API}/repos/{}/releases/assets/{asset_id}",
                self.repo
            ))
            .header("Authorization", &self.auth())
            .header("Accept", "application/octet-stream")
            .call()
            .map_err(CoreError::runtime)?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            let text = response.into_body().read_to_string().unwrap_or_default();
            return Err(self.http_error(status, &text));
        }
        let bytes = response
            .into_body()
            .read_to_vec()
            .map_err(CoreError::runtime)?;
        serde_json::from_slice(&bytes).map_err(CoreError::runtime)
    }

    fn download_asset_file(&self, asset_id: i64, out: &Path) -> Result<u64, CoreError> {
        let mut last_error = String::new();
        for attempt in 0..MAX_RETRIES {
            let response = self
                .agent
                .get(&format!(
                    "{API}/repos/{}/releases/assets/{asset_id}",
                    self.repo
                ))
                .header("Authorization", &self.auth())
                .header("Accept", "application/octet-stream")
                .call();
            let response = match response {
                Ok(response) => response,
                Err(error) => {
                    last_error = format!("{error}");
                    std::thread::sleep(Duration::from_secs(15 * (attempt as u64 + 1)));
                    continue;
                }
            };
            let status = response.status().as_u16();
            if !(200..300).contains(&status) {
                let text = response.into_body().read_to_string().unwrap_or_default();
                if matches!(status, 401 | 403 | 404 | 409 | 422) {
                    return Err(self.http_error(status, &text));
                }
                last_error = self.http_error(status, &text).to_string();
                std::thread::sleep(Duration::from_secs(15 * (attempt as u64 + 1)));
                continue;
            }
            let mut file = File::create(out).map_err(CoreError::os)?;
            let mut reader = response.into_body().into_reader();
            match std::io::copy(&mut reader, &mut file) {
                Ok(bytes) => return Ok(bytes),
                Err(error) => {
                    last_error = format!("{error}");
                    std::thread::sleep(Duration::from_secs(15 * (attempt as u64 + 1)));
                }
            }
        }
        Err(CoreError::runtime(format!(
            "Failed to download after {MAX_RETRIES} attempts: {last_error}"
        )))
    }

    pub fn download(&self, backup_name: &str, dest: &Path) -> Result<(), CoreError> {
        let release = self.get_release_by_tag(backup_name)?.ok_or_else(|| {
            CoreError::runtime(format!("No GitHub Release found for tag: {backup_name}"))
        })?;
        fs::create_dir_all(dest).map_err(CoreError::os)?;
        let mut assets = release
            .get("assets")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assets.sort_by_key(|asset| asset.get("name").map(text::py_str).unwrap_or_default());
        let total: u64 = assets
            .iter()
            .filter_map(|asset| asset.get("size").and_then(Value::as_u64))
            .sum::<u64>()
            .max(1);
        let mut done = 0u64;
        let mut progress = EventProgress::new("download");
        for asset in assets {
            let Some(id) = asset.get("id").and_then(Value::as_i64) else {
                continue;
            };
            let name = asset.get("name").map(text::py_str).unwrap_or_default();
            if manifest::clean_backup_name(&name) != name {
                return Err(CoreError::runtime(format!(
                    "Unsafe asset name in release: {name:?}"
                )));
            }
            let bytes = self.download_asset_file(id, &dest.join(&name))?;
            done += bytes;
            progress.emit(done, Some(total), "bytes", Some(&name), true);
        }

        let groups = part_groups(dest)?;
        for (base, mut parts) in groups {
            parts.sort();
            let mut out = File::create(dest.join(&base)).map_err(CoreError::os)?;
            for part in parts {
                let mut input = File::open(&part).map_err(CoreError::os)?;
                std::io::copy(&mut input, &mut out).map_err(CoreError::os)?;
                let _ = fs::remove_file(part);
            }
        }
        Ok(())
    }

    pub fn delete_old(&self, max_keep: i64) -> Result<(), CoreError> {
        let mut releases: Vec<Value> = self
            .list_releases()?
            .into_iter()
            .filter(|release| {
                release
                    .get("tag_name")
                    .map(text::py_str)
                    .map(|tag| {
                        tag.starts_with("termux_backup_") && manifest::valid_backup_name(&tag)
                    })
                    .unwrap_or(false)
            })
            .collect();
        releases.sort_by_key(|release| {
            std::cmp::Reverse(
                release
                    .get("created_at")
                    .map(text::py_str)
                    .unwrap_or_default(),
            )
        });
        let keep = max_keep.max(1) as usize;
        for release in releases.into_iter().skip(keep) {
            let Some(id) = release.get("id").and_then(Value::as_i64) else {
                continue;
            };
            let tag = release
                .get("tag_name")
                .map(text::py_str)
                .unwrap_or_default();
            self.api(
                "DELETE",
                &format!("/repos/{}/releases/{id}", self.repo),
                None,
            )?;
            std::thread::sleep(Duration::from_secs(1));
            self.api(
                "DELETE",
                &format!(
                    "/repos/{}/git/refs/tags/{}",
                    self.repo,
                    encode_segment(&tag)
                ),
                None,
            )?;
        }
        Ok(())
    }

    pub fn delete_backup(&self, backup_name: &str) -> Result<(), CoreError> {
        let release = self.get_release_by_tag(backup_name)?.ok_or_else(|| {
            CoreError::file_not_found(format!(
                "Backup '{backup_name}' not found in GitHub Releases."
            ))
        })?;
        let id = release
            .get("id")
            .and_then(Value::as_i64)
            .ok_or_else(|| CoreError::runtime("GitHub release has no id"))?;
        self.api(
            "DELETE",
            &format!("/repos/{}/releases/{id}", self.repo),
            None,
        )?;
        self.api(
            "DELETE",
            &format!("/repos/{}/git/refs/tags/{backup_name}", self.repo),
            None,
        )?;
        Ok(())
    }
}

pub fn part_name(base: &str, index: u32) -> String {
    format!("{base}.part{index:04}")
}

/// Upload a tar stream in 200 MiB parts, deleting each part after upload.
pub struct PartWriter {
    github: GitHubStorage,
    release_id: i64,
    work_dir: PathBuf,
    base: String,
    index: u32,
    total: u64,
    hasher: Sha256,
    file: Option<BufWriter<File>>,
    path: Option<PathBuf>,
    size: u64,
    limit: u64,
    next_note: u64,
    progress: EventProgress,
}

impl PartWriter {
    fn new(github: GitHubStorage, release_id: i64, work_dir: PathBuf, base: String) -> Self {
        Self {
            github,
            release_id,
            work_dir,
            base,
            index: 0,
            total: 0,
            hasher: Sha256::new(),
            file: None,
            path: None,
            size: 0,
            limit: CHUNK_SIZE,
            next_note: 0,
            progress: EventProgress::new("upload"),
        }
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    pub fn hexdigest(&self) -> String {
        format!("{:x}", self.hasher.clone().finalize())
    }

    fn flush_part(&mut self, final_part: bool) -> std::io::Result<()> {
        let Some(mut file) = self.file.take() else {
            return Ok(());
        };
        file.flush()?;
        drop(file);
        let Some(mut path) = self.path.take() else {
            return Ok(());
        };
        if final_part && self.index == 0 {
            let plain = self.work_dir.join(&self.base);
            fs::rename(&path, &plain)?;
            path = plain;
        }
        let result = self.github.upload_asset(self.release_id, &path);
        let _ = fs::remove_file(&path);
        result.map_err(std::io::Error::other)?;
        self.index += 1;
        self.size = 0;
        Ok(())
    }

    pub fn finish(mut self) -> Result<(u64, String), CoreError> {
        if self.file.is_some() {
            self.flush_part(true)
                .map_err(|error| CoreError::plain(error.to_string()))?;
        }
        let digest = format!("{:x}", self.hasher.finalize());
        Ok((self.total, digest))
    }

    pub fn abort(&mut self) {
        self.file = None;
        if let Some(path) = self.path.take() {
            let _ = fs::remove_file(path);
        }
    }
}

impl Write for PartWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let mut position = 0usize;
        while position < data.len() {
            if self.file.is_some() && self.size >= self.limit {
                self.flush_part(false)?;
            }
            if self.file.is_none() {
                let path = self.work_dir.join(part_name(&self.base, self.index));
                let file = File::create(&path)?;
                self.file = Some(BufWriter::new(file));
                self.path = Some(path);
                self.size = 0;
            }
            let remaining = (self.limit - self.size) as usize;
            let take = remaining.min(data.len() - position);
            if let Some(file) = self.file.as_mut() {
                file.write_all(&data[position..position + take])?;
            }
            self.size += take as u64;
            self.total += take as u64;
            self.hasher.update(&data[position..position + take]);
            position += take;
        }
        if self.total >= self.next_note {
            self.next_note = self.total + 8 * 1024 * 1024;
            self.progress
                .emit(self.total, None, "bytes", Some(&self.base), false);
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn storage() -> GitHubStorage {
        GitHubStorage {
            token: "x".to_string(),
            repo: "owner/repo".to_string(),
            branch: "main".to_string(),
            agent: agent(),
            stream: None,
        }
    }

    fn test_storage() -> GitHubStorage {
        storage()
    }

    #[test]
    fn part_names_are_four_digits() {
        assert_eq!(part_name("home.tar.gz", 0), "home.tar.gz.part0000");
        assert_eq!(part_name("home.tar.gz", 1234), "home.tar.gz.part1234");
    }

    #[test]
    fn github_errors_match_python_classes() {
        let storage = storage();
        assert!(
            storage
                .http_error(409, r#"{"message":"Git Repository is empty."}"#)
                .contains("GitHub repository is empty")
        );
        assert!(
            storage
                .http_error(401, "{}")
                .contains("Invalid GitHub token")
        );
        assert!(
            storage
                .http_error(403, "{}")
                .contains("GitHub denied access")
        );
        assert!(
            storage
                .http_error(403, r#"{"message":"API rate limit exceeded"}"#)
                .contains("rate limit")
        );
        assert!(
            storage
                .http_error(404, "{}")
                .contains("repository not found")
        );
        assert!(
            storage
                .http_error(500, "server exploded")
                .contains("GitHub API error 500")
        );
    }

    #[test]
    fn empty_part_writer_has_empty_digest() {
        let writer = PartWriter::new(
            storage(),
            1,
            std::env::temp_dir(),
            "home.tar.gz".to_string(),
        );
        let (total, digest) = writer.finish().unwrap();
        assert_eq!(total, 0);
        assert_eq!(digest, format!("{:x}", Sha256::new().finalize()));
    }

    #[test]
    fn validates_repo_and_encodes_segments() {
        assert!(valid_repo("owner/repo"));
        assert!(!valid_repo("owner"));
        assert!(!valid_repo("owner/repo/extra"));
        assert!(!valid_repo("../repo"));
        assert_eq!(encode_segment("a b/c"), "a%20b%2Fc");
    }

    #[test]
    fn non_200_release_list_is_an_error() {
        assert!(releases_from_response(401, r#"{"message":"Bad credentials"}"#).is_err());
        assert_eq!(releases_from_response(200, "[]").unwrap().len(), 0);
    }

    #[test]
    fn existing_asset_must_match_size() {
        assert!(asset_matches(&json!({"size": 10}), 10));
        assert!(!asset_matches(&json!({"size": 9}), 10));
        assert!(!asset_matches(&json!({}), 10));
    }

    #[test]
    fn split_uses_small_chunks_without_extra_memory() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("a.bin");
        fs::write(&src, b"0123456789abcdefghij").unwrap();
        let storage = test_storage();
        let parts = storage.split_file_at(&src, temp.path(), 8).unwrap();
        assert_eq!(parts.len(), 3);
        assert_eq!(fs::read(&parts[1]).unwrap(), b"89abcdef");
    }

    #[test]
    fn missing_download_dir_is_an_error() {
        assert!(part_groups(&tempfile::tempdir().unwrap().path().join("nope")).is_err());
    }
}
