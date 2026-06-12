use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_repr::{Deserialize_repr, Serialize_repr};

use crate::types::{
    Location, Range, TextDocumentIdentifier, TextDocumentPositionParams, TextEdit,
    WorkDoneProgressParams,
};

pub trait Request: DeserializeOwned {
    const METHOD: &'static str;
    type Response: Serialize;
}

#[derive(Deserialize, Debug)]
pub struct HoverParams {
    #[serde(flatten)]
    pub position: TextDocumentPositionParams,
    #[serde(flatten)]
    pub work_done: WorkDoneProgressParams,
}
impl Request for HoverParams {
    const METHOD: &'static str = "textDocument/hover";
    type Response = Hover;
}

#[derive(Serialize, Default)]
pub struct Hover {
    pub contents: HoverContents, // TODO: can be MarkedString/MarkupContents/...
    pub range: Option<Range>,
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum HoverContents {
    MarkedString(MarkedString),
    MarkedStrings(Vec<MarkedString>),
    MarkupContent(MarkupContent),
}
impl From<String> for HoverContents {
    fn from(value: String) -> Self {
        Self::MarkedString(MarkedString::String(value))
    }
}
impl<'a> From<&'a str> for HoverContents {
    fn from(value: &'a str) -> Self {
        Self::MarkedString(MarkedString::String(value.to_owned()))
    }
}
impl Default for HoverContents {
    fn default() -> Self {
        Self::MarkedString(MarkedString::default())
    }
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum MarkedString {
    String(String),
    Code { language: String, value: String },
}
impl Default for MarkedString {
    fn default() -> Self {
        Self::String(String::default())
    }
}

#[derive(Deserialize)]
pub struct CompletionParams {
    #[serde(flatten)]
    pub position: TextDocumentPositionParams,
    #[serde(flatten)]
    pub work_done: WorkDoneProgressParams,
    // PartialResultParams
    pub context: Option<CompletionContext>,
}
impl Request for CompletionParams {
    const METHOD: &'static str = "textDocument/completion";
    type Response = Vec<CompletionItem>;
}

#[derive(Deserialize)]
pub struct CompletionContext {
    pub triggerKind: CompletionTriggerKind,
    pub triggerCharacter: Option<String>,
}

#[derive(Deserialize_repr)]
#[repr(u8)]
pub enum CompletionTriggerKind {
    Invoked = 1,
    TriggerCharacter = 2,
    TriggerForIncompleteCompletions = 3,
}

#[derive(Serialize)]
pub struct CompletionItem {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<CompletionItemKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labelDetails: Option<CompletionItemLabelDetails>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation: Option<MarkupContent>,
    pub preselect: bool,
}

#[derive(Serialize)]
pub struct MarkupContent {
    pub kind: MarkupKind,
    pub value: String,
}
impl MarkupContent {
    pub fn markdown(s: impl Into<String>) -> Self {
        Self {
            kind: MarkupKind::Markdown,
            value: s.into(),
        }
    }
}

#[derive(Serialize)]
pub enum MarkupKind {
    #[serde(rename = "plaintext")]
    PlainText,
    #[serde(rename = "markdown")]
    Markdown,
}

#[derive(Serialize)]
pub struct CompletionItemLabelDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Serialize_repr)]
#[repr(u8)]
pub enum CompletionItemKind {
    Text = 1,
    Method = 2,
    Function = 3,
    Constructor = 4,
    Field = 5,
    Variable = 6,
    Class = 7,
    Interface = 8,
    Module = 9,
    Property = 10,
    Unit = 11,
    Value = 12,
    Enum = 13,
    Keyword = 14,
    Snippet = 15,
    Color = 16,
    File = 17,
    Reference = 18,
    Folder = 19,
    EnumMember = 20,
    Constant = 21,
    Struct = 22,
    Event = 23,
    Operator = 24,
    TypeParameter = 25,
}

#[derive(Deserialize)]
pub struct DefinitionParams {
    #[serde(flatten)]
    pub position: TextDocumentPositionParams,
    // WorkDoneProgressParams, PartialResultParams
}
impl Request for DefinitionParams {
    const METHOD: &'static str = "textDocument/definition";
    type Response = Option<Location>;
}
#[derive(Deserialize, Debug)]
pub struct DocumentFormattingParams {
    pub textDocument: TextDocumentIdentifier,
    pub options: FormattingOptions,
}
impl Request for DocumentFormattingParams {
    const METHOD: &'static str = "textDocument/formatting";
    type Response = Option<Vec<TextEdit>>;
}

#[derive(Deserialize, Debug)]
pub struct FormattingOptions {
    // ...
}
