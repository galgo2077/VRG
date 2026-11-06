use std::{
    collections::BTreeMap,
    ffi::OsStr,
    io::{Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
    time::UNIX_EPOCH,
};

use cap_std::{ambient_authority, fs::Dir};
use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};
use sha2::{Digest, Sha256};

use crate::{
    config::{RootConfig, VrgConfig},
    contracts::{LinkKind, LinkRef, SourceDocument, SourceRef},
    error::{VrgError, VrgResult},
};

const DEFAULT_MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;
const MAX_FRONTMATTER_BYTES: usize = 64 * 1024;
const FRONTMATTER_FIELDS: &[&str] = &["title", "aliases", "tags", "date", "created", "modified"];
const PRIVATE_BOOLEAN_FIELDS: &[&str] = &[
    "private",
    "is_private",
    "personal",
    "confidential",
    "secret",
    "sensitive",
    "restricted",
    "exclude",
    "excluded",
    "exclude_from_vrg",
    "exclude_from_index",
    "no_index",
    "ignore_for_vrg",
    "vrg_exclude",
    "private_note",
    "private_content",
];
const PRIVATE_CLASSIFICATION_FIELDS: &[&str] = &[
    "privacy",
    "classification",
    "sensitivity",
    "visibility",
    "access",
];

#[derive(Clone, Debug)]
pub struct ScanBatch {
    pub documents: Vec<SourceDocument>,
    pub files_seen: usize,
    pub excluded: usize,
    pub issues: Vec<ScanIssue>,
}

#[derive(Clone, Debug)]
pub struct ScanIssue {
    pub root_id: String,
    pub relative_path: Option<String>,
    /// Stable code only; never contains source text or an OS error payload.
    pub code: String,
}

#[derive(Debug)]
struct PendingLink {
    target: String,
    kind: LinkKind,
    start_byte: usize,
    end_byte: usize,
    label: String,
}

/// Read configured Markdown roots without changing them.
///
/// Any issue that could make the enumerated manifest incomplete is returned in `issues`; callers
/// must not publish a generation from a batch with issues. Policy exclusions and symbolic links
/// are counted as excluded entries and are never followed.
pub fn scan_roots(config: &VrgConfig) -> VrgResult<ScanBatch> {
    config.validate()?;
    let mut batch = ScanBatch {
        documents: Vec::new(),
        files_seen: 0,
        excluded: 0,
        issues: Vec::new(),
    };

    for root in &config.roots {
        let canonical_root = match root.path.canonicalize() {
            Ok(path) if path.is_dir() => path,
            Ok(_) => {
                batch
                    .issues
                    .push(issue(&root.id, None, "root_not_directory"));
                continue;
            }
            Err(_) => {
                batch.issues.push(issue(&root.id, None, "root_unavailable"));
                continue;
            }
        };

        ensure_root_separation(config, &root.id, &canonical_root)?;

        let dir = match Dir::open_ambient_dir(&canonical_root, ambient_authority()) {
            Ok(dir) => dir,
            Err(_) => {
                batch.issues.push(issue(&root.id, None, "root_open_failed"));
                continue;
            }
        };
        scan_dir(root, &dir, Path::new(""), &mut batch)?;
    }

    batch.documents.sort_by(|a, b| a.source.cmp(&b.source));
    Ok(batch)
}

/// Read one bounded Markdown source under a configured root using capability-relative opens.
pub fn read_source(
    config: &VrgConfig,
    root_id: &str,
    relative_path: &str,
    max_bytes: usize,
) -> VrgResult<SourceDocument> {
    read_source_impl(config, root_id, relative_path, max_bytes, true)
}
pub(crate) fn read_index_source(
    config: &VrgConfig,
    root_id: &str,
    relative_path: &str,
    max_bytes: usize,
) -> VrgResult<SourceDocument> {
    read_source_impl(config, root_id, relative_path, max_bytes, false)
}
pub(crate) fn current_revision(config: &VrgConfig, source: &SourceRef) -> VrgResult<String> {
    if let Some(revision) =
        crate::projection::updates::direct_revision(config, &source.root_id, &source.relative_path)?
    {
        return Ok(revision);
    }
    Ok(read_source(
        config,
        &source.root_id,
        &source.relative_path,
        DEFAULT_MAX_SOURCE_BYTES,
    )?
    .revision)
}
fn read_source_impl(
    config: &VrgConfig,
    root_id: &str,
    relative_path: &str,
    max_bytes: usize,
    verify_dependencies: bool,
) -> VrgResult<SourceDocument> {
    config.validate()?;
    if max_bytes == 0 {
        return Err(VrgError::InvalidConfig(
            "source read limit must be positive".into(),
        ));
    }
    if let Some(document) =
        crate::projection::updates::read_direct_source(config, root_id, relative_path, max_bytes)?
    {
        return Ok(document);
    }
    if max_bytes == 0 || max_bytes > DEFAULT_MAX_SOURCE_BYTES {
        return Err(VrgError::InvalidConfig(format!(
            "max_bytes must be between 1 and {DEFAULT_MAX_SOURCE_BYTES}"
        )));
    }
    let root = config
        .roots
        .iter()
        .find(|root| root.id == root_id)
        .ok_or_else(|| VrgError::AccessDenied("root ID is not configured".into()))?;
    let relative = validate_relative_path(relative_path)?;
    if is_excluded(&relative, root) || !is_markdown(&relative) {
        return Err(VrgError::AccessDenied(
            "source path is excluded by policy".into(),
        ));
    }

    let root_path = root
        .path
        .canonicalize()
        .map_err(|_| VrgError::AccessDenied("configured root is unavailable".into()))?;
    ensure_root_separation(config, &root.id, &root_path)?;
    let root_dir = Dir::open_ambient_dir(&root_path, ambient_authority())
        .map_err(|_| VrgError::AccessDenied("configured root cannot be opened".into()))?;
    let (parent, file_name) = open_parent_without_symlinks(&root_dir, &relative)?;
    let metadata = parent
        .symlink_metadata(&file_name)
        .map_err(|_| VrgError::AccessDenied("source path is unavailable".into()))?;
    if metadata.file_type().is_symlink() {
        return Err(VrgError::SymlinkRejected(relative_path.into()));
    }
    if !metadata.is_file() {
        return Err(VrgError::AccessDenied("source path is not a file".into()));
    }
    if metadata.len() > max_bytes as u64 {
        return Err(VrgError::AccessDenied(
            "source exceeds the read limit".into(),
        ));
    }
    let mut file = parent
        .open(&file_name)
        .map_err(|_| VrgError::AccessDenied("source path cannot be opened".into()))?;
    if read_private_frontmatter(&mut file)? {
        return Err(VrgError::AccessDenied(
            "source is excluded by privacy policy".into(),
        ));
    }
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.by_ref()
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        return Err(VrgError::AccessDenied(
            "source exceeds the read limit".into(),
        ));
    }
    let document = parse_document(
        root_id,
        relative_path,
        bytes,
        metadata.modified().ok().map(|time| time.into_std()),
    )?;
    if verify_dependencies {
        crate::projection::updates::authorize_output(
            config,
            root_id,
            relative_path,
            &document.revision,
        )?;
    }
    Ok(document)
}

fn scan_dir(
    root: &RootConfig,
    dir: &Dir,
    relative_dir: &Path,
    batch: &mut ScanBatch,
) -> VrgResult<()> {
    let entries = match dir.entries() {
        Ok(entries) => entries,
        Err(_) => {
            batch.issues.push(issue(
                &root.id,
                safe_relative_string(relative_dir),
                "directory_read_failed",
            ));
            return Ok(());
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                batch.issues.push(issue(
                    &root.id,
                    safe_relative_string(relative_dir),
                    "directory_entry_failed",
                ));
                continue;
            }
        };
        let name = entry.file_name();
        let Some(name_text) = name.to_str() else {
            batch.excluded += 1;
            batch.issues.push(issue(
                &root.id,
                safe_relative_string(relative_dir),
                "non_utf8_name",
            ));
            continue;
        };
        let relative = relative_dir.join(name_text);
        let relative_text = match safe_relative_string(&relative) {
            Some(value) => value,
            None => {
                batch.excluded += 1;
                batch
                    .issues
                    .push(issue(&root.id, None, "unsafe_relative_path"));
                continue;
            }
        };

        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => {
                batch
                    .issues
                    .push(issue(&root.id, Some(relative_text), "file_type_failed"));
                continue;
            }
        };
        if file_type.is_symlink() {
            batch.excluded += 1;
            continue;
        }
        if is_excluded(&relative, root) {
            batch.excluded += 1;
            continue;
        }
        if file_type.is_dir() {
            match dir.open_dir(&name) {
                Ok(child) => scan_dir(root, &child, &relative, batch)?,
                Err(_) => batch.issues.push(issue(
                    &root.id,
                    Some(relative_text),
                    "directory_open_failed",
                )),
            }
            continue;
        }
        if !file_type.is_file() || !is_markdown(&relative) {
            batch.excluded += 1;
            continue;
        }

        batch.files_seen += 1;
        let metadata = match dir.metadata(&name) {
            Ok(metadata) => metadata,
            Err(_) => {
                batch
                    .issues
                    .push(issue(&root.id, Some(relative_text), "metadata_failed"));
                continue;
            }
        };
        if metadata.len() > DEFAULT_MAX_SOURCE_BYTES as u64 {
            batch
                .issues
                .push(issue(&root.id, Some(relative_text), "source_too_large"));
            continue;
        }
        let mut file = match dir.open(&name) {
            Ok(file) => file,
            Err(_) => {
                batch
                    .issues
                    .push(issue(&root.id, Some(relative_text), "source_open_failed"));
                continue;
            }
        };
        match read_private_frontmatter(&mut file) {
            Ok(true) => {
                batch.excluded += 1;
                continue;
            }
            Ok(false) => {}
            Err(VrgError::Markdown(_)) => {
                batch.issues.push(issue(
                    &root.id,
                    Some(relative_text),
                    "invalid_markdown_metadata",
                ));
                continue;
            }
            Err(_) => {
                batch
                    .issues
                    .push(issue(&root.id, Some(relative_text), "source_read_failed"));
                continue;
            }
        }
        if file.seek(SeekFrom::Start(0)).is_err() {
            batch
                .issues
                .push(issue(&root.id, Some(relative_text), "source_read_failed"));
            continue;
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        if file
            .by_ref()
            .take(DEFAULT_MAX_SOURCE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .is_err()
        {
            batch
                .issues
                .push(issue(&root.id, Some(relative_text), "source_read_failed"));
            continue;
        }
        if bytes.len() > DEFAULT_MAX_SOURCE_BYTES {
            batch
                .issues
                .push(issue(&root.id, Some(relative_text), "source_too_large"));
            continue;
        }
        match parse_document(
            &root.id,
            &relative_text,
            bytes,
            metadata.modified().ok().map(|time| time.into_std()),
        ) {
            Ok(document) => batch.documents.push(document),
            Err(VrgError::AccessDenied(_)) => batch.excluded += 1,
            Err(VrgError::Markdown(_)) => batch.issues.push(issue(
                &root.id,
                Some(relative_text),
                "invalid_markdown_metadata",
            )),
            Err(_) => {
                batch
                    .issues
                    .push(issue(&root.id, Some(relative_text), "source_parse_failed"))
            }
        }
    }
    Ok(())
}

fn parse_document(
    root_id: &str,
    relative_path: &str,
    bytes: Vec<u8>,
    modified: Option<std::time::SystemTime>,
) -> VrgResult<SourceDocument> {
    let raw_markdown = String::from_utf8(bytes.clone())
        .map_err(|_| VrgError::Markdown("source is not valid UTF-8".into()))?;
    let (frontmatter, body) = split_frontmatter(&raw_markdown)?;
    if is_private_frontmatter(&frontmatter) {
        return Err(VrgError::AccessDenied(
            "source is excluded by privacy policy".into(),
        ));
    }
    let safe_frontmatter = allowlisted_frontmatter(frontmatter);
    let fallback_title = Path::new(relative_path)
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or("Untitled");
    let title = safe_frontmatter
        .get("title")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(fallback_title)
        .to_owned();
    let (body_text, links) = parse_body(body);
    let revision = hex::encode(Sha256::digest(&bytes));
    let modified_unix_ms = modified
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64);

    Ok(SourceDocument {
        source: SourceRef {
            root_id: root_id.to_owned(),
            relative_path: relative_path.to_owned(),
        },
        revision,
        title,
        raw_markdown,
        body_text,
        frontmatter: safe_frontmatter,
        links,
        modified_unix_ms,
    })
}

fn split_frontmatter(markdown: &str) -> VrgResult<(BTreeMap<String, serde_json::Value>, &str)> {
    let Some(after_open) = markdown
        .strip_prefix("---\n")
        .or_else(|| markdown.strip_prefix("---\r\n"))
    else {
        return Ok((BTreeMap::new(), markdown));
    };
    let mut byte_cursor = markdown.len() - after_open.len();
    for line in after_open.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed == "---" || trimmed == "..." {
            let yaml_start = markdown.len() - after_open.len();
            let yaml_end = byte_cursor;
            let yaml = markdown.get(yaml_start..yaml_end).unwrap_or_default();
            let body_start = byte_cursor + line.len();
            let body = markdown.get(body_start..).ok_or_else(|| {
                VrgError::Markdown("frontmatter delimiter is not on a UTF-8 boundary".into())
            })?;
            let values = parse_frontmatter_values(yaml.as_bytes())?;
            return Ok((values, body));
        }
        byte_cursor += line.len();
    }
    Err(VrgError::Markdown(
        "frontmatter opening delimiter has no closing delimiter".into(),
    ))
}

/// Read only the YAML prefix needed to determine whether the whole note is private. A private
/// note's Markdown body is not loaded, indexed, returned, or persisted.
pub(crate) fn read_private_frontmatter(reader: &mut impl Read) -> VrgResult<bool> {
    const OPENERS: [&[u8]; 4] = [
        b"---\n",
        b"---\r\n",
        b"\xef\xbb\xbf---\n",
        b"\xef\xbb\xbf---\r\n",
    ];
    let mut opener = Vec::with_capacity(5);
    loop {
        let mut byte = [0; 1];
        if reader.read(&mut byte)? == 0 {
            return Ok(false);
        }
        opener.push(byte[0]);
        if OPENERS
            .iter()
            .any(|candidate| *candidate == opener.as_slice())
        {
            break;
        }
        if !OPENERS
            .iter()
            .any(|candidate| candidate.starts_with(&opener))
        {
            return Ok(false);
        }
    }

    let mut consumed = opener.len();
    let mut yaml = Vec::new();
    loop {
        let line = read_frontmatter_line(reader)?;
        let Some(line) = line else {
            return Err(VrgError::Markdown(
                "frontmatter opening delimiter has no closing delimiter".into(),
            ));
        };
        consumed = consumed.saturating_add(line.len());
        if consumed > MAX_FRONTMATTER_BYTES {
            return Err(VrgError::Markdown(
                "frontmatter exceeds the size limit".into(),
            ));
        }
        match trim_line_ending(&line) {
            b"---" | b"..." => {
                return Ok(is_private_frontmatter(&parse_frontmatter_values(&yaml)?));
            }
            _ => yaml.extend_from_slice(&line),
        }
    }
}

fn read_frontmatter_line(reader: &mut impl Read) -> VrgResult<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let mut byte = [0; 1];
        if reader.read(&mut byte)? == 0 {
            return Ok((!line.is_empty()).then_some(line));
        }
        line.push(byte[0]);
        if line.len() > MAX_FRONTMATTER_BYTES {
            return Err(VrgError::Markdown(
                "frontmatter exceeds the size limit".into(),
            ));
        }
        if byte[0] == b'\n' {
            return Ok(Some(line));
        }
    }
}

fn trim_line_ending(line: &[u8]) -> &[u8] {
    let without_lf = line.strip_suffix(b"\n").unwrap_or(line);
    without_lf.strip_suffix(b"\r").unwrap_or(without_lf)
}

fn parse_frontmatter_values(yaml: &[u8]) -> VrgResult<BTreeMap<String, serde_json::Value>> {
    let yaml = std::str::from_utf8(yaml)
        .map_err(|_| VrgError::Markdown("frontmatter is invalid UTF-8".into()))?;
    if yaml.trim().is_empty() {
        Ok(BTreeMap::new())
    } else {
        serde_yaml_ng::from_str::<BTreeMap<String, serde_json::Value>>(yaml)
            .map_err(|_| VrgError::Markdown("frontmatter is invalid".into()))
    }
}

fn allowlisted_frontmatter(
    values: BTreeMap<String, serde_json::Value>,
) -> BTreeMap<String, serde_json::Value> {
    let mut safe = BTreeMap::new();
    for (key, value) in values {
        let normalized = key.to_ascii_lowercase();
        if FRONTMATTER_FIELDS.contains(&normalized.as_str()) {
            safe.entry(normalized).or_insert(value);
        }
    }
    safe
}

/// Private frontmatter is excluded before a document can enter an index or be returned by
/// `read_source`. Classification fields and explicit boolean exclusion flags are case-insensitive.
fn is_private_frontmatter(values: &BTreeMap<String, serde_json::Value>) -> bool {
    values.iter().any(|(key, value)| {
        let key = normalize_metadata_key(key);
        if PRIVATE_BOOLEAN_FIELDS.contains(&key.as_str()) {
            is_truthy_marker(value)
        } else if PRIVATE_CLASSIFICATION_FIELDS.contains(&key.as_str()) {
            has_private_classification(value)
        } else if matches!(key.as_str(), "tag" | "tags") {
            has_private_tag(value)
        } else if key == "vrg" {
            match value {
                serde_json::Value::Object(fields) => {
                    let nested = fields
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect();
                    is_private_frontmatter(&nested)
                }
                _ => false,
            }
        } else {
            false
        }
    })
}

fn normalize_metadata_key(key: &str) -> String {
    key.trim()
        .to_ascii_lowercase()
        .replace('-', "_")
        .replace(' ', "_")
}

fn is_truthy_marker(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::String(value) => {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "true" | "yes" | "on" | "1" | "si" | "sí"
            ) || contains_private_marker(value)
        }
        serde_json::Value::Number(value) => value.as_i64() == Some(1),
        _ => false,
    }
}

fn has_private_classification(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::String(value) => {
            contains_private_marker(value)
                || matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "true" | "yes" | "on" | "1"
                )
        }
        serde_json::Value::Array(values) => values.iter().any(has_private_classification),
        _ => false,
    }
}

fn has_private_tag(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(value) => contains_private_marker(value),
        serde_json::Value::Array(values) => values.iter().any(has_private_tag),
        _ => false,
    }
}

fn contains_private_marker(value: &str) -> bool {
    value
        .split(|character: char| !character.is_ascii_alphanumeric())
        .map(str::to_ascii_lowercase)
        .any(|marker| {
            matches!(
                marker.as_str(),
                "private"
                    | "privado"
                    | "privada"
                    | "personal"
                    | "confidential"
                    | "confidencial"
                    | "secret"
                    | "secreto"
                    | "secreta"
                    | "sensitive"
                    | "sensible"
                    | "restricted"
                    | "restringido"
                    | "restringida"
            )
        })
}

fn parse_body(body: &str) -> (String, Vec<LinkRef>) {
    let mut options = Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_WIKILINKS
        | Options::ENABLE_FOOTNOTES;
    options.insert(Options::ENABLE_GFM);
    let parser = Parser::new_ext(body, options).into_offset_iter();
    let mut body_text = String::new();
    let mut links = Vec::new();
    let mut pending: Vec<PendingLink> = Vec::new();

    for (event, range) in parser {
        match event {
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                ..
            }) => {
                let kind = if matches!(link_type, LinkType::WikiLink { .. }) {
                    LinkKind::Wiki
                } else {
                    LinkKind::Markdown
                };
                pending.push(PendingLink {
                    target: dest_url.into_string(),
                    kind,
                    start_byte: range.start,
                    end_byte: range.end,
                    label: String::new(),
                });
            }
            Event::Start(Tag::Image { dest_url, .. }) => pending.push(PendingLink {
                target: dest_url.into_string(),
                kind: LinkKind::Embed,
                start_byte: range.start,
                end_byte: range.end,
                label: String::new(),
            }),
            Event::Text(text) | Event::Code(text) => {
                body_text.push_str(&text);
                if let Some(link) = pending.last_mut() {
                    link.label.push_str(&text);
                }
            }
            Event::Html(text) | Event::InlineHtml(text) => body_text.push_str(&text),
            Event::SoftBreak | Event::HardBreak => body_text.push('\n'),
            Event::End(TagEnd::Link) | Event::End(TagEnd::Image) => {
                if let Some(mut link) = pending.pop() {
                    link.end_byte = range.end;
                    if let Some(link_ref) = finish_link(link) {
                        links.push(link_ref);
                    }
                }
                body_text.push(' ');
            }
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::Item
                | TagEnd::TableCell
                | TagEnd::TableRow,
            ) => body_text.push('\n'),
            _ => {}
        }
    }
    links.sort_by_key(|link| (link.start_byte, link.end_byte));
    (body_text, links)
}

fn finish_link(link: PendingLink) -> Option<LinkRef> {
    let (target, alias_from_target) = if link.kind == LinkKind::Wiki {
        match link.target.split_once('|') {
            Some((target, alias)) => (target.trim().to_owned(), Some(alias.trim().to_owned())),
            None => (link.target.trim().to_owned(), None),
        }
    } else {
        (link.target.trim().to_owned(), None)
    };
    if target.is_empty() || is_external_target(&target) {
        return None;
    }
    let (target, heading) = match target.split_once('#') {
        Some((target, heading)) => (
            target.to_owned(),
            (!heading.trim().is_empty()).then(|| heading.trim().to_owned()),
        ),
        None => (target, None),
    };
    let alias = alias_from_target
        .or_else(|| (!link.label.trim().is_empty()).then(|| link.label.trim().to_owned()));
    Some(LinkRef {
        target,
        alias,
        heading,
        kind: link.kind,
        start_byte: link.start_byte.min(u32::MAX as usize) as u32,
        end_byte: link.end_byte.min(u32::MAX as usize) as u32,
    })
}

fn open_parent_without_symlinks<'a>(root: &'a Dir, relative: &Path) -> VrgResult<(Dir, String)> {
    let mut components = relative.components().peekable();
    let mut current = root.try_clone()?;
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            return Err(VrgError::PathEscape(relative.display().to_string()));
        };
        let name_text = name
            .to_str()
            .ok_or_else(|| VrgError::PathEscape("non-UTF-8 path".into()))?;
        if components.peek().is_none() {
            return Ok((current, name_text.to_owned()));
        }
        let metadata = current
            .symlink_metadata(name)
            .map_err(|_| VrgError::AccessDenied("source directory is unavailable".into()))?;
        if metadata.file_type().is_symlink() {
            return Err(VrgError::SymlinkRejected(name_text.to_owned()));
        }
        if !metadata.is_dir() {
            return Err(VrgError::AccessDenied(
                "source parent is not a directory".into(),
            ));
        }
        current = current
            .open_dir(name)
            .map_err(|_| VrgError::AccessDenied("source directory cannot be opened".into()))?;
    }
    Err(VrgError::PathEscape("empty relative path".into()))
}

fn validate_relative_path(value: &str) -> VrgResult<PathBuf> {
    let path = Path::new(value);
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(VrgError::PathEscape(value.into()));
    }
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(VrgError::PathEscape(value.into()));
        }
    }
    if value.contains('\\')
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(VrgError::PathEscape(value.into()));
    }
    Ok(path.to_path_buf())
}

fn safe_relative_string(path: &Path) -> Option<String> {
    if path.as_os_str().is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    for component in path.components() {
        let Component::Normal(part) = component else {
            return None;
        };
        parts.push(part.to_str()?);
    }
    Some(parts.join("/"))
}

pub(crate) fn is_excluded(path: &Path, root: &RootConfig) -> bool {
    let Some(relative) = safe_relative_string(path) else {
        return true;
    };
    if root
        .allowed_files
        .as_ref()
        .is_some_and(|allowed| !allowed.iter().any(|name| name == &relative))
    {
        return true;
    }
    if root.exclusions.iter().any(|pattern| {
        let pattern = pattern.trim_matches('/');
        !pattern.is_empty() && (relative == pattern || relative.starts_with(&format!("{pattern}/")))
    }) {
        return true;
    }
    for component in path.components() {
        let Component::Normal(name) = component else {
            return true;
        };
        let name = name.to_string_lossy().to_ascii_lowercase();
        if name.starts_with('.')
            || matches!(
                name.as_str(),
                "node_modules" | "target" | "cache" | "caches" | "tmp" | "temp"
            )
            || [
                "backup",
                "backups",
                "secret",
                "secrets",
                "credential",
                "credentials",
                "oauth",
                "token",
                "tokens",
                "password",
                "private-key",
                "private_key",
                "id_rsa",
            ]
            .iter()
            .any(|needle| name.contains(needle))
            || name.ends_with(".bak")
            || name.ends_with(".backup")
            || name.ends_with(".old")
            || name.ends_with('~')
        {
            return true;
        }
    }
    false
}

pub(crate) fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

fn is_external_target(target: &str) -> bool {
    let lower = target.trim().to_ascii_lowercase();
    lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("mailto:")
        || lower.starts_with("data:")
        || lower.starts_with("//")
        || lower.starts_with('#')
        || lower.split_once(':').is_some_and(|(scheme, _)| {
            !scheme.is_empty()
                && scheme
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.'))
        })
}

fn ensure_root_separation(config: &VrgConfig, root_id: &str, root_path: &Path) -> VrgResult<()> {
    if paths_overlap(root_path, &comparable_path(&config.data_dir)?) {
        return Err(VrgError::InvalidConfig(format!(
            "derived data directory overlaps source root {root_id}"
        )));
    }
    Ok(())
}

fn issue(root_id: &str, relative_path: Option<String>, code: &str) -> ScanIssue {
    ScanIssue {
        root_id: root_id.to_owned(),
        relative_path,
        code: code.to_owned(),
    }
}

fn comparable_path(path: &Path) -> VrgResult<PathBuf> {
    if path.exists() {
        return path
            .canonicalize()
            .map_err(|_| VrgError::InvalidConfig("cannot canonicalize data directory".into()));
    }
    let mut cursor = path.to_path_buf();
    let mut suffix = Vec::new();
    while !cursor.exists() {
        let name = cursor
            .file_name()
            .ok_or_else(|| VrgError::InvalidConfig("invalid data directory path".into()))?
            .to_os_string();
        suffix.push(name);
        if !cursor.pop() {
            return Err(VrgError::InvalidConfig(
                "invalid data directory path".into(),
            ));
        }
    }
    let mut result = cursor
        .canonicalize()
        .map_err(|_| VrgError::InvalidConfig("cannot canonicalize data directory parent".into()))?;
    for part in suffix.iter().rev() {
        result.push(part);
    }
    Ok(result)
}

fn paths_overlap(root: &Path, data: &Path) -> bool {
    root == data || root.starts_with(data) || data.starts_with(root)
}
