use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::types::{
    Diagnostic, DocumentUri, Integer, TextDocumentContentChangeEvent, TextDocumentIdentifier,
    TextDocumentItem, VersionedTextDocumentIdentifier,
};

pub trait Notification: DeserializeOwned {}
pub trait ServerNotification: Serialize {
    const METHOD: &'static str;
}

#[derive(Deserialize)]
pub struct DidOpenTextDocumentParams {
    pub textDocument: TextDocumentItem,
}
impl Notification for DidOpenTextDocumentParams {}

#[derive(Deserialize, Debug)]
pub struct DidChangeTextDocumentParams {
    pub textDocument: VersionedTextDocumentIdentifier,
    pub contentChanges: Vec<TextDocumentContentChangeEvent>,
}
impl Notification for DidChangeTextDocumentParams {}

#[derive(Deserialize, Debug)]
pub struct DidSaveTextDocumentParams {
    pub textDocument: TextDocumentIdentifier,
    pub text: Option<String>,
}
impl Notification for DidSaveTextDocumentParams {}

#[derive(Serialize, Debug)]
pub struct PublishDiagnosticsParams {
    pub uri: DocumentUri,
    pub version: Option<Integer>,
    pub diagnostics: Vec<Diagnostic>,
}
impl ServerNotification for PublishDiagnosticsParams {
    const METHOD: &'static str = "textDocument/publishDiagnostics";
}
