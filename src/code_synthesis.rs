//! Deterministic structural explanations. Source text is evidence, never instructions.
//! No model inference, runtime verification, or conversation ingestion occurs here.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

const MAX_FACTS: usize = 24;
const MAX_TEXT: usize = 240;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EvidenceFact {
    pub line: usize,
    pub text: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct SourceFacts {
    pub responsibilities: Vec<EvidenceFact>,
    pub interfaces: Vec<EvidenceFact>,
    pub dependencies: Vec<EvidenceFact>,
    pub persistence: Vec<EvidenceFact>,
    pub configuration: Vec<EvidenceFact>,
    pub conditions: Vec<EvidenceFact>,
    pub concurrency: Vec<EvidenceFact>,
    pub errors: Vec<EvidenceFact>,
    pub integrations: Vec<EvidenceFact>,
    pub calls: Vec<EvidenceFact>,
    pub entrypoint: bool,
    /// Token-level body fingerprint includes literals, guards and defaults.
    pub behavioral_sha256: String,
    /// Documentation evidence may change independently of code behavior.
    pub documentation_sha256: String,
    pub omitted_facts: usize,
}

#[derive(Clone, Debug)]
pub struct SynthesisSource {
    pub source_id: String,
    pub relative_path: String,
    pub revision: String,
    pub facts: SourceFacts,
}

#[derive(Clone, Debug)]
pub struct ModuleSynthesis {
    pub markdown: String,
    pub compact_memory: String,
    pub significance_sha256: String,
}

fn hash(bytes: &[u8]) -> String { hex::encode(Sha256::digest(bytes)) }
fn bounded(text: &str, limit: usize) -> String { text.chars().take(limit).collect() }
fn code(text: &str) -> String {
    format!("`{}`", bounded(text, MAX_TEXT).replace('`', "'").replace(['\n', '\r'], " "))
}
fn add(out: &mut Vec<EvidenceFact>, omitted: &mut usize, line: usize, text: &str) {
    if out.len() < MAX_FACTS { out.push(EvidenceFact { line, text: bounded(text, MAX_TEXT) }); }
    else { *omitted += 1; }
}

/// Conservative tokenizer: drops whitespace and line/block comments, preserves
/// quoted literals verbatim. Comments inside strings cannot change the fingerprint.
/// A '#' is a comment only at line start or after whitespace and never before '['.
pub fn behavioral_fingerprint(text: &str) -> String {
    let guessed = if text.starts_with("#!/") || text.lines().any(|line| line.trim_start().starts_with("def ") || line.trim_start().starts_with("from ")) { "source.py" } else { "source.rs" };
    behavioral_fingerprint_for_path(text, guessed)
}

pub fn behavioral_fingerprint_for_path(text: &str, relative_path: &str) -> String {
    let extension = relative_path.rsplit('.').next().unwrap_or("");
    if extension == "rs" && (text.contains("r#\"") || text.contains("r##") || text.contains("br#")) {
        return hash(text.as_bytes());
    }
    let python = matches!(extension, "py" | "pyi");
    let shell = matches!(extension, "sh" | "bash" | "zsh" | "fish") || text.starts_with("#!/bin/sh") || text.starts_with("#!/bin/bash");
    let hash_comments = python || shell || matches!(extension, "rb" | "r" | "R" | "pl" | "ps1");
    let slash_comments = !python && !shell && !matches!(extension, "rb" | "r" | "R" | "pl");
    let semantic_newlines = python || shell;
    let bytes = text.as_bytes();
    let mut normalized = Vec::new();
    let mut at = 0;
    let mut block_depth = 0usize;
    let mut line_blank = true;
    let mut indentation = 0usize;
    while at < bytes.len() {
        let current = bytes[at];
        if block_depth > 0 {
            if bytes.get(at..at+2) == Some(b"/*") { block_depth += 1; at += 2; }
            else if bytes.get(at..at+2) == Some(b"*/") { block_depth -= 1; at += 2; }
            else { at += 1; }
            continue;
        }
        if current.is_ascii_whitespace() {
            if current == b'\n' {
                if semantic_newlines && !line_blank { normalized.push(255); }
                line_blank = true; indentation = 0;
            } else if line_blank { indentation += if current == b'\t' { 8 - indentation % 8 } else { 1 }; }
            // Separate punctuation tokens when removing trivia would otherwise
            // turn `+ +` into `++`, or `> >` into `>>`.
            if !normalized.is_empty() {
                let mut next = at + 1;
                while next < bytes.len() && bytes[next].is_ascii_whitespace() { next += 1; }
                if let (Some(previous), Some(following)) = (normalized.last(), bytes.get(next)) {
                    let pair = [*previous, *following];
                    if [*b"++",*b"--",*b"->",*b"=>",*b"==",*b"!=",*b"<=",*b">=",*b"&&",*b"||",*b"<<",*b">>",*b"**",*b"//",*b"..",*b"::",*b"?.",*b"??",*b"+=",*b"-=",*b"*=",*b"/="].contains(&pair) { normalized.push(0); }
                }
            }
            at += 1; continue;
        }
        if slash_comments && bytes.get(at..at+2) == Some(b"/*") { block_depth = 1; at += 2; continue; }
        if slash_comments && bytes.get(at..at+2) == Some(b"//") || hash_comments && current == b'#' && bytes.get(at+1) != Some(&b'[') && (line_blank || at > 0 && bytes[at-1].is_ascii_whitespace()) {
            while at < bytes.len() && bytes[at] != b'\n' { at += 1; }
            continue;
        }
        if python && line_blank { normalized.push(254); normalized.extend_from_slice(&indentation.to_le_bytes()); }
        line_blank = false;
        if current == b'"' || current == b'\'' || current == b'`' {
            // Rust lifetimes ('a, 'static) are identifier tokens, not quotes.
            let mut lifetime_end = at + 1;
            while lifetime_end < bytes.len() && (bytes[lifetime_end].is_ascii_alphanumeric() || bytes[lifetime_end] == b'_') { lifetime_end += 1; }
            let lifetime_name = &bytes[at+1..lifetime_end];
            let lifetime = extension == "rs" && current == b'\'' && (lifetime_name.len() == 1 || lifetime_name == b"static" || lifetime_name == b"de")
                && bytes.get(lifetime_end) != Some(&b'\'');
            if !lifetime {
                let delimiter = current;
                let triple = bytes.get(at..at+3).is_some_and(|s| s == [delimiter, delimiter, delimiter]);
                let width = if triple { 3 } else { 1 };
                normalized.extend_from_slice(&bytes[at..at+width]); at += width;
                while at < bytes.len() {
                    if bytes[at] == b'\\' {
                        normalized.push(bytes[at]);at += 1;
                        if at < bytes.len() { normalized.push(bytes[at]);at += 1; }
                    } else if bytes.get(at..at+width).is_some_and(|s| s.iter().all(|b| *b == delimiter)) {
                        normalized.extend_from_slice(&bytes[at..at+width]);at += width;break;
                    } else { normalized.push(bytes[at]);at += 1; }
                }
                normalized.push(0); continue;
            }
        }
        if current.is_ascii_alphanumeric() || current == b'_' || current >= 128 {
            while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_' || bytes[at] >= 128) {
                normalized.push(bytes[at]); at += 1;
            }
            normalized.push(0);
        } else { normalized.push(current); at += 1; }
    }
    hash(&normalized)
}

pub fn analyze_source(bytes: &[u8]) -> SourceFacts {
    analyze_source_for_path(bytes, "")
}

pub fn analyze_source_for_path(bytes: &[u8], relative_path: &str) -> SourceFacts {
    let text = std::str::from_utf8(bytes).unwrap_or("");
    let fingerprint = if relative_path.is_empty() { behavioral_fingerprint(text) } else { behavioral_fingerprint_for_path(text, relative_path) };
    let mut facts = SourceFacts { behavioral_sha256: fingerprint, ..Default::default() };
    let mut docs = String::new();
    for (index, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim(); let number = index + 1;
        if line.starts_with("#!/") { facts.entrypoint = true; }
        if line.starts_with("//!") || line.starts_with("///") || index < 12 && line.starts_with('#') && !line.starts_with("#!") && !line.starts_with("#[") && !line.starts_with("#include ") && !line.starts_with("#define ") {
            let comment = line.trim_start_matches('/').trim_start_matches('!').trim_start_matches('#').trim();
            if !comment.is_empty() {
                add(&mut facts.responsibilities, &mut facts.omitted_facts, number, comment);
                docs.push_str(comment);docs.push('\n');
            }
            continue;
        }
        if line.starts_with("//") || line.starts_with('#') && !line.starts_with("#[") && !line.starts_with("#include ") { continue; }
        let declaration = line.starts_with("pub ") || line.starts_with("pub(") || line.starts_with("export ") || line.starts_with("def ") || line.starts_with("async def ") || line.starts_with("class ") || line.starts_with("interface ") || line.starts_with("function ");
        if declaration {
            let signature = line.split('{').next().unwrap_or(line).trim_end_matches(':').trim();
            add(&mut facts.interfaces, &mut facts.omitted_facts, number, signature);
        }
        if line.starts_with("use ") || line.starts_with("import ") || line.starts_with("from ") || line.starts_with("#include ") || line.contains("require(") {
            add(&mut facts.dependencies, &mut facts.omitted_facts, number, line);
        }
        if line.contains("fn main(") || line.contains("__name__") && line.contains("__main__") || line.starts_with("#!/") { facts.entrypoint = true; }
        let tokens = line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).filter(|s| !s.is_empty()).collect::<BTreeSet<_>>();
        if tokens.iter().any(|t| ["rusqlite","sqlite3","CREATE","serialize","deserialize","write_text","write_bytes","OpenOptions","File","save","persist","commit"].contains(t)) {
            add(&mut facts.persistence, &mut facts.omitted_facts, number, line);
        }
        let uppercase_assignment = line.split_once('=').is_some_and(|(left,_)| {
            let left = left.trim(); !left.is_empty() && left.chars().any(|c| c.is_ascii_uppercase())
                && left.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        });
        if uppercase_assignment || tokens.iter().any(|t| ["config","configuration","getenv","environ","env","default","defaults","const","static","allow_cache","enabled"].contains(t)) {
            add(&mut facts.configuration, &mut facts.omitted_facts, number, line);
        }
        if line.starts_with("if ") || line.starts_with("elif ") || line.starts_with("else if ") || line.starts_with("while ") || line.starts_with("match ") || line.contains(" if ") {
            add(&mut facts.conditions, &mut facts.omitted_facts, number, line);
        }
        if tokens.iter().any(|t| ["async","await","Mutex","RwLock","Semaphore","tokio","thread","watcher","notify","Event","channel"].contains(t)) {
            add(&mut facts.concurrency, &mut facts.omitted_facts, number, line);
        }
        if tokens.iter().any(|t| ["Err","Result","Error","Exception","raise","except","panic","unwrap","Conflict","AccessDenied"].contains(t)) {
            add(&mut facts.errors, &mut facts.omitted_facts, number, line);
        }
        if tokens.iter().any(|t| ["reqwest","http","https","subprocess","Command","mcp","WebSocket","fetch","requests"].contains(t)) {
            add(&mut facts.integrations, &mut facts.omitted_facts, number, line);
        }
        if !declaration && line.contains('(') && !line.starts_with("//") && facts.calls.len() < MAX_FACTS {
            // This is call-site text, not a resolved call graph or execution-order proof.
            add(&mut facts.calls, &mut facts.omitted_facts, number, line);
        }
    }
    facts.documentation_sha256 = hash(docs.as_bytes()); facts
}


/// Canonical lexical spelling for compact evidence. Whitespace outside literals
/// is formatting; quoted literal bytes and separated operator tokens survive.
fn compact_fact_text(text: &str, relative_path: &str) -> String {
    let extension=relative_path.rsplit('.').next().unwrap_or("");
    let hash_comments=matches!(extension, "py" | "pyi" | "sh" | "bash" | "zsh" | "rb" | "r" | "R" | "pl");
    let slash_comments=!hash_comments;
    let chars = text.chars().collect::<Vec<_>>();
    let mut tokens = Vec::<String>::new(); let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        if c.is_whitespace() { at += 1; continue; }
        if hash_comments && c == '#' || slash_comments && c == '/' && chars.get(at+1) == Some(&'/') { break; }
        let start = at;
        if c == '\'' || c == '"' || c == '`' {
            at += 1;
            while at < chars.len() {
                if chars[at] == '\\' { at = (at + 2).min(chars.len()); }
                else if chars[at] == c { at += 1; break; }
                else { at += 1; }
            }
        } else if c.is_alphanumeric() || c == '_' {
            at += 1;
            while at < chars.len() && (chars[at].is_alphanumeric() || chars[at] == '_') { at += 1; }
        } else {
            at += 1;
            if at < chars.len() {
                let pair = [c, chars[at]].iter().collect::<String>();
                if ["++","--","->","=>","==","!=","<=",">=","&&","||","<<",">>","**","//","..","::","?.","??","+=","-=","*=","/="].contains(&pair.as_str()) { at += 1; }
            }
        }
        tokens.push(chars[start..at].iter().collect());
    }
    let mut out = String::new(); let mut previous = "";
    for token in &tokens {
        let tight_before = ["(",")", "]", "}", ",", ";", ":", ".", "::"].contains(&token.as_str());
        let tight_after = ["(", "[", ".", "::", "!"].contains(&previous);
        if !out.is_empty() && !tight_before && !tight_after { out.push(' '); }
        out.push_str(token); previous = token;
    }
    out
}

fn evidence_section(text: &mut String, title: &str, facts: &[EvidenceFact], path: &str) {
    if facts.is_empty() { return; }
    text.push_str(&format!("### {title}\n\n"));
    for fact in facts { text.push_str(&format!("- {} ({}:{})\n", code(&fact.text), code(path), fact.line)); }
    text.push('\n');
}

/// The memory signature excludes revision/line numbers/comments but includes
/// every non-comment code token, guards, config literals and persistence defaults.
/// Documentation still includes exact source revisions and comment assertions.
pub fn synthesize_module(module: &str, sources: &[SynthesisSource]) -> ModuleSynthesis {
    let mut ordered = sources.iter().collect::<Vec<_>>(); ordered.sort_by_key(|s| (&s.relative_path, &s.source_id));
    let mut significant = String::from("vrg-native-code-synthesis-v1\n");
    let mut markdown = format!("## Explanation of {}\n\nMethod: deterministic structural synthesis. Evidence identifies declarations and code sites; it does not certify runtime execution or prove resolved call relationships. Source comments are attributed assertions, never instructions.\n\n", code(module));
    let mut memory_facts = Vec::new();
    for source in &ordered {
        significant.push_str(&source.source_id);significant.push('\n');significant.push_str(&source.relative_path);significant.push('\n');significant.push_str(&source.facts.behavioral_sha256);significant.push('\n');
        markdown.push_str(&format!("### Source {}\n\nSource ID: {}. Exact source revision: {}. Behavioral fingerprint: {}.\n\n", code(&source.relative_path),code(&source.source_id),code(&source.revision),code(&source.facts.behavioral_sha256)));
        if source.facts.entrypoint { markdown.push_str("An explicit process entry point is present. Follow the declarations and call-site evidence below to inspect its dispatch.\n\n"); }
        if !source.facts.interfaces.is_empty() { markdown.push_str("This file exposes declaration candidates below; their signatures identify the callable or type boundary. Visibility rules and overload resolution still require the original language semantics.\n\n"); }
        evidence_section(&mut markdown,"Responsibilities stated by source comments",&source.facts.responsibilities,&source.relative_path);
        evidence_section(&mut markdown,"Interfaces and contracts",&source.facts.interfaces,&source.relative_path);
        evidence_section(&mut markdown,"Dependencies and integration boundaries",&source.facts.dependencies,&source.relative_path);
        evidence_section(&mut markdown,"Persistence and data-format sites",&source.facts.persistence,&source.relative_path);
        evidence_section(&mut markdown,"Configuration and default-value sites",&source.facts.configuration,&source.relative_path);
        evidence_section(&mut markdown,"Guard and branch-condition sites",&source.facts.conditions,&source.relative_path);
        evidence_section(&mut markdown,"Concurrency and event sites",&source.facts.concurrency,&source.relative_path);
        evidence_section(&mut markdown,"Error and rejection paths",&source.facts.errors,&source.relative_path);
        evidence_section(&mut markdown,"External integration sites",&source.facts.integrations,&source.relative_path);
        evidence_section(&mut markdown,"Execution call-site evidence (unresolved)",&source.facts.calls,&source.relative_path);
        if source.facts.omitted_facts > 0 { markdown.push_str(&format!("{} additional lexical facts omitted from this bounded explanation; consult the complete source.\n\n",source.facts.omitted_facts)); }
        let interface_names = source.facts.interfaces.iter().take(2).map(|f| bounded(&f.text.split(|c:char| !(c.is_alphanumeric() || c == '_')).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" "),80)).collect::<Vec<_>>().join("; ");
        memory_facts.push(format!("{}: interfaces [{}]; deps {}; persistence {}; concurrency {}; errors {}; entrypoint {}",bounded(&source.relative_path,100),interface_names,source.facts.dependencies.len(),source.facts.persistence.len(),source.facts.concurrency.len(),source.facts.errors.len(),source.facts.entrypoint));
    }
    markdown.push_str("### Known errors and verified fixes\n\nNo runtime failure or resolved incident is inferred from identifiers, test names or comments. Consult separately attributed verified operational evidence for such claims.\n\n");
    let significance_sha256=hash(significant.as_bytes());
    let mut compact_memory=format!("# Structural memory\n\nProducer: native_mechanical\nMethod: deterministic_structural_synthesis\nModule: {}\nBehavioral signature: {}\nSource count: {}\nStatus: proposed; exact-source expansion required; runtime behavior not verified.\n\n",bounded(module,180),significance_sha256,ordered.len());
    // Preserve useful current values and branch text, rather than only a digest
    // or inventory counts. These are bounded lexical facts, not runtime claims.
    let mut candidates = Vec::<String>::new();
    let categories: [(&str, fn(&SourceFacts) -> &[EvidenceFact]); 6] = [
        ("Declared configuration/default candidate", |f| &f.configuration),
        ("Observed guard/branch code", |f| &f.conditions),
        ("Observed persistence code", |f| &f.persistence),
        ("Observed error/rejection code", |f| &f.errors),
        ("Observed integration code", |f| &f.integrations),
        ("Observed call-site code (unresolved)", |f| &f.calls),
    ];
    let mut omitted = 0usize;
    let mut evidence_groups = Vec::<Vec<String>>::new();
    for (label, select) in categories {
        let mut facts = ordered.iter().flat_map(|source| select(&source.facts).iter().map(move |fact| (source, fact))).collect::<Vec<_>>();
        facts.sort_by_key(|(source, fact)| (&source.relative_path, &fact.text));
        omitted += facts.len().saturating_sub(6);
        let mut group = Vec::new();
        for (source, fact) in facts.into_iter().take(6) {
            let normalized = compact_fact_text(&fact.text, &source.relative_path);
            group.push(format!("{label}: {} => {}",bounded(&source.relative_path,100),bounded(&normalized,200)));
        }
        evidence_groups.push(group);
    }
    for round in 0..6 {
        for group in &evidence_groups {
            if let Some(fact) = group.get(round) { candidates.push(fact.clone()); }
        }
    }
    candidates.extend(memory_facts.into_iter().map(|fact| format!("Declared interface inventory: {fact}")));
    // Reserve bytes for an explicit omission notice. File provenance lives in
    // HEAD grants and current module docs, independently of this stable text.
    for fact in candidates {
        if compact_memory.len()+fact.len()+5 > 3850 { omitted += 1; continue; }
        compact_memory.push_str("- "); compact_memory.push_str(&fact); compact_memory.push('\n');
    }
    if omitted > 0 { compact_memory.push_str(&format!("Omitted lexical facts/inventory entries: {omitted}; expand current source for complete conditions.\n")); }
    ModuleSynthesis { markdown, compact_memory, significance_sha256 }
}
