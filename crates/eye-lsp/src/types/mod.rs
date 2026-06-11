#![allow(non_snake_case, dead_code)]

pub mod notification;
pub mod request;

use std::path::Path;

use error::span::TSpan;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct Uri(String);
impl Uri {
    pub fn from_path(path: &Path) -> Self {
        // TODO: this could probably be handled more gracefully
        let ending = if path.is_dir() { "/main.eye" } else { "" };
        Self(format!("file://{}{ending}", path.to_string_lossy()))
    }

    pub fn path(&self) -> &Path {
        Path::new(self.0.strip_prefix("file://").unwrap_or(&self.0))
    }
}
type DocumentUri = Uri;
type Integer = i32;
type UInteger = u32;

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Id {
    Number(i32),
    String(String),
}

#[derive(Deserialize)]
pub struct Initialize {
    #[serde(rename = "processId")]
    pub process_id: Option<i32>,
    #[serde(rename = "rootPath")]
    pub root_path: Option<String>,
    #[serde(rename = "rootUri")]
    pub root_uri: Option<DocumentUri>,
}

#[derive(Serialize)]
pub struct InitializeResult {
    pub capabilities: ServerCapabilities,
    #[serde(rename = "serverInfo")]
    pub server_info: ServerInfo,
}

#[derive(Serialize)]
pub struct ServerCapabilities {
    pub positionEncoding: String,
    pub hoverProvider: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completionProvider: Option<CompletionOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub textDocumentSync: Option<TextDocumentSyncOptions>,
    pub definitionProvider: bool,
    pub documentFormattingProvider: bool,
    // ...
}

#[derive(Serialize)]
pub struct CompletionOptions {
    pub triggerCharacters: Vec<String>,
    // resolveProvider
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completionItem: Option<CompletionItem>,
}

#[derive(Serialize)]
pub struct CompletionItem {
    pub labelDetailsSupport: Option<bool>,
}

#[derive(Serialize)]
pub struct TextDocumentSyncOptions {
    pub openClose: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change: Option<TextDocumentSyncKind>,
    pub save: bool,
}

#[derive(serde_repr::Serialize_repr)]
#[repr(i32)]
pub enum TextDocumentSyncKind {
    None = 0,
    Full = 1,
    Incremental = 2,
}

#[derive(Serialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}
impl Range {
    // TODO: these aren't computed in UTF-16
    pub fn from_span(span: TSpan, src: &str) -> Self {
        Self {
            start: Position::from_offset(span.start, src),
            end: Position::from_offset(span.end, src),
        }
    }

    pub fn to_span(&self, src: &str) -> TSpan {
        debug_assert!(self.start.line <= self.end.line);
        let mut lines_to_skip = self.start.line;
        let mut line_start_offset = 0;
        for (i, &b) in src.as_bytes().iter().enumerate() {
            if lines_to_skip == 0 {
                break;
            }
            if b == b'\n' {
                lines_to_skip -= 1;
                line_start_offset = i + 1;
            }
        }
        let mut lines_to_skip = self.end.line - self.start.line;
        let mut line_end_offset = line_start_offset;
        for (i, &b) in src.as_bytes()[line_start_offset..].iter().enumerate() {
            if lines_to_skip == 0 {
                break;
            }
            if b == b'\n' {
                lines_to_skip -= 1;
                line_end_offset = line_start_offset + i + 1;
            }
        }
        tracing::debug!(
            "Converting range {:?} -> {} .. {}",
            self,
            line_start_offset as u32 + self.start.character,
            line_end_offset as u32 + self.end.character,
        );
        TSpan::new(
            line_start_offset as u32 + self.start.character,
            line_end_offset as u32 + self.end.character,
        )
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}
impl Position {
    pub fn to_offset(&self, mut src: &str) -> u32 {
        let mut offset = 0;
        for _ in 0..self.line {
            let Some(i) = src.find('\n') else {
                return src.len() as u32;
            };
            let i = i + 1;
            offset += i as u32;
            src = &src[i..];
        }
        let mut chars = src.chars();
        for _ in 0..self.character {
            let Some(c) = chars.next() else {
                break;
            };
            if c == '\n' {
                break;
            }
            offset += c.len_utf8() as u32;
        }

        offset
    }

    pub fn from_offset(offset: u32, src: &str) -> Self {
        let position = error::calculate_position(src, offset);
        Self {
            line: position.line,
            character: position.column,
        }
    }
}

#[derive(Deserialize, Debug)]
pub struct TextDocumentPositionParams {
    pub textDocument: TextDocumentIdentifier,
    pub position: Position,
}

#[derive(Serialize, Deserialize)]
pub struct TextDocumentItem {
    pub uri: DocumentUri,
    pub languageId: String,
    pub version: Integer,
    pub text: String,
}

#[derive(Deserialize, Debug)]
pub struct WorkDoneProgressParams {
    workDoneToken: Option<ProgressToken>,
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum ProgressToken {
    String(String),
    Int(i32),
}

#[derive(Deserialize, Debug)]
pub struct TextDocumentIdentifier {
    pub uri: DocumentUri,
}

#[derive(Deserialize, Debug)]
pub struct VersionedTextDocumentIdentifier {
    #[serde(flatten)]
    pub identifier: TextDocumentIdentifier,
    pub version: i32,
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum TextDocumentContentChangeEvent {
    TextDocumentContentChangePartial { range: Range, text: String },
    TextDocumentContentChangeWholeDocument { text: String },
}

#[derive(Serialize, Debug)]
pub struct Diagnostic {
    pub range: Range,
    pub severity: Option<DiagnosticSeverity>,
    pub code: Option<String>,
    pub codeDescription: Option<CodeDescription>,
    pub source: Option<String>,
    pub message: String,
    // ...
}

#[derive(Debug, Clone, Copy)]
pub enum DiagnosticSeverity {
    Error = 1,
    Warning = 2,
    Information = 3,
    Hint = 4,
}
impl Serialize for DiagnosticSeverity {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_i32(*self as i32)
    }
}

#[derive(Serialize, Debug)]
pub struct CodeDescription {
    pub href: Uri,
}

#[derive(Serialize)]
pub struct Location {
    pub uri: DocumentUri,
    pub range: Range,
}

#[derive(Serialize, Deserialize)]
pub struct TextEdit {
    pub range: Range,
    pub newText: String,
}
