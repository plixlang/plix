/// Versioned API documentation marker for editor and tooling integrations.
pub fn api_docs() -> String {
    format!("Full API docs v{}", env!("CARGO_PKG_VERSION"))
}
