//! Port of @t3tools/shared delimitedPreview.ts `parseDelimitedPreview` and of
//! extendedDocumentPreview.ts `openNotebook`, for the Files panel's table and notebook previews.
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DelimitedLimits {
    pub max_rows: usize,
    pub max_columns: usize,
    pub max_cell_length: usize,
}
impl Default for DelimitedLimits {
    fn default() -> Self {
        Self {
            max_rows: 100,
            max_columns: 30,
            max_cell_length: 2000,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DelimitedPreview {
    pub rows: Vec<Vec<String>>,
    pub truncated: bool,
}

fn utf16_prefix(cell: &str, units: usize) -> &str {
    let mut count = 0;
    for (index, c) in cell.char_indices() {
        count += c.len_utf16();
        if count > units {
            return &cell[..index];
        }
    }
    cell
}

/// RFC 4180-ish: quoted cells, doubled quotes, CR/LF/CRLF rows, a leading BOM skipped. Lengths
/// are counted in UTF-16 units, as the TypeScript does, and never split a surrogate pair.
pub fn parse_delimited_preview(text: &str, delimiter: char, limits: DelimitedLimits) -> DelimitedPreview {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut cell_units = 0usize;
    let mut quoted = false;
    let mut truncated = false;
    let body = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut row_has_content = false;
    let end_cell = |row: &mut Vec<String>, cell: &mut String, units: &mut usize, truncated: &mut bool| {
        if *units > limits.max_cell_length {
            *cell = utf16_prefix(cell, limits.max_cell_length).to_owned();
            *truncated = true;
        }
        if row.len() < limits.max_columns {
            row.push(std::mem::take(cell));
        } else {
            cell.clear();
            *truncated = true;
        }
        *units = 0;
    };
    let mut chars = body.char_indices().peekable();
    while let Some((_, c)) = chars.next() {
        row_has_content = true;
        if c == '"' {
            if quoted && chars.peek().map(|(_, n)| *n) == Some('"') {
                if cell_units <= limits.max_cell_length {
                    cell.push('"');
                    cell_units += 1;
                } else {
                    truncated = true;
                }
                chars.next();
                continue;
            }
            if quoted || cell.is_empty() {
                quoted = !quoted;
                continue;
            }
        }
        if !quoted && (c == delimiter || c == '\n' || c == '\r') {
            end_cell(&mut row, &mut cell, &mut cell_units, &mut truncated);
            if c != delimiter {
                rows.push(std::mem::take(&mut row));
                row_has_content = false;
                if c == '\r' && chars.peek().map(|(_, n)| *n) == Some('\n') {
                    chars.next();
                }
                if rows.len() == limits.max_rows {
                    let rest = chars.peek().is_some();
                    return DelimitedPreview {
                        rows,
                        truncated: truncated || rest,
                    };
                }
            }
        } else if cell_units <= limits.max_cell_length {
            cell.push(c);
            cell_units += c.len_utf16();
        } else {
            truncated = true;
        }
    }
    if row_has_content {
        end_cell(&mut row, &mut cell, &mut cell_units, &mut truncated);
        rows.push(row);
    }
    DelimitedPreview {
        rows,
        truncated: truncated || quoted,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellKind {
    Markdown,
    Code,
    Raw,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputKind {
    Text,
    Image,
    Html,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotebookOutput {
    pub kind: OutputKind,
    pub text: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotebookCell {
    pub kind: CellKind,
    pub source: String,
    pub outputs: Vec<NotebookOutput>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notebook {
    pub language: String,
    pub cells: Vec<NotebookCell>,
}

pub const NOTEBOOK_MAX_BYTES: usize = 8 * 1024 * 1024;

/// The reader's refusal, in its client English (a translation key).
pub type NotebookError = &'static str;

fn notebook_text(value: Option<&Value>, limit: usize) -> Result<String, NotebookError> {
    let text = match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts.iter().filter_map(Value::as_str).collect(),
        _ => String::new(),
    };
    if text.encode_utf16().count() > limit {
        return Err("A notebook cell is too large to preview.");
    }
    Ok(text)
}

fn base64_image_syntax(base64: &str) -> bool {
    let body = base64.trim_end_matches('=');
    base64.len() - body.len() <= 2
        && body.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
        && base64.len() % 4 != 1
        && (!base64.ends_with('=') || base64.len().is_multiple_of(4))
}

/// `openNotebook`: version 4 notebooks of up to 500 cells and 100 outputs per cell. Saved
/// outputs only; nothing runs.
pub fn open_notebook(text: &str) -> Result<Notebook, NotebookError> {
    if text.len() > NOTEBOOK_MAX_BYTES {
        return Err("Notebook preview limited to files under 8 MB.");
    }
    let unsupported = "Preview supports version 4 notebooks with up to 500 cells.";
    let notebook: Value = serde_json::from_str(text).map_err(|_| unsupported)?;
    let cells = notebook.get("cells").and_then(Value::as_array);
    let version = notebook.get("nbformat").and_then(Value::as_f64);
    let Some(cells) = cells.filter(|cells| version == Some(4.0) && cells.len() <= 500) else {
        return Err(unsupported);
    };
    let language: String = notebook
        .pointer("/metadata/language_info/name")
        .map(|name| match name {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        })
        .unwrap_or_else(|| "text".into())
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '#' | '-'))
        .take(40)
        .collect();
    let cells = cells
        .iter()
        .map(|cell| {
            let outputs = cell.get("outputs").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
            if outputs.len() > 100 {
                return Err("A notebook cell has too many outputs to preview.");
            }
            let kind = match cell.get("cell_type").and_then(Value::as_str) {
                Some("markdown") => CellKind::Markdown,
                Some("code") => CellKind::Code,
                _ => CellKind::Raw,
            };
            let mut shown = Vec::new();
            'outputs: for output in outputs {
                let data = output.get("data");
                for mime in ["image/png", "image/jpeg", "image/gif", "image/webp"] {
                    if let Some(value) = data.and_then(|d| d.get(mime)).filter(|v| !v.is_null()) {
                        let base64: String = notebook_text(Some(value), 4 * 1024 * 1024)?
                            .chars()
                            .filter(|c| !c.is_whitespace())
                            .collect();
                        if base64_image_syntax(&base64) {
                            shown.push(NotebookOutput {
                                kind: OutputKind::Image,
                                text: format!("data:{mime};base64,{base64}"),
                            });
                            continue 'outputs;
                        }
                    }
                }
                let html = notebook_text(data.and_then(|d| d.get("text/html")), 256_000)?;
                if !html.trim().is_empty() {
                    shown.push(NotebookOutput { kind: OutputKind::Html, text: html });
                    continue;
                }
                let source = output
                    .get("text")
                    .or_else(|| data.and_then(|d| d.get("text/plain")))
                    .or_else(|| output.get("traceback"));
                let text = notebook_text(source, 256_000)?;
                if !text.trim().is_empty() {
                    shown.push(NotebookOutput { kind: OutputKind::Text, text });
                }
            }
            Ok(NotebookCell {
                kind,
                source: notebook_text(cell.get("source"), 256_000)?,
                outputs: shown,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Notebook { language, cells })
}

/// NotebookPreview's source file name for a code cell's highlighting.
pub fn notebook_cell_language(language: &str) -> String {
    match language {
        "python" => "py",
        "javascript" => "js",
        "typescript" => "ts",
        "julia" => "jl",
        other => other,
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(preview: &DelimitedPreview) -> Vec<Vec<&str>> {
        preview.rows.iter().map(|r| r.iter().map(String::as_str).collect()).collect()
    }

    #[test]
    fn delimited_rows_follow_quotes_and_line_endings() {
        let preview = parse_delimited_preview("\u{feff}a,\"b,c\",\"d\"\"e\"\r\n1,2,3\n", ',', DelimitedLimits::default());
        assert_eq!(rows(&preview), vec![vec!["a", "b,c", "d\"e"], vec!["1", "2", "3"]]);
        assert!(!preview.truncated);
        let tabs = parse_delimited_preview("x\ty\nz", '\t', DelimitedLimits::default());
        assert_eq!(rows(&tabs), vec![vec!["x", "y"], vec!["z"]]);
        assert!(parse_delimited_preview("\"open", ',', DelimitedLimits::default()).truncated);
    }

    #[test]
    fn delimited_limits_truncate_rows_columns_and_cells() {
        let limits = DelimitedLimits { max_rows: 2, max_columns: 2, max_cell_length: 3 };
        let preview = parse_delimited_preview("abcdef,b,c\n1\n2\n", ',', limits);
        assert_eq!(rows(&preview), vec![vec!["abc", "b"], vec!["1"]]);
        assert!(preview.truncated);
        let emoji = parse_delimited_preview("a😀b", ',', DelimitedLimits { max_cell_length: 2, ..limits });
        assert_eq!(rows(&emoji), vec![vec!["a"]], "a surrogate pair is never split");
    }

    #[test]
    fn notebooks_keep_saved_outputs_only() {
        let notebook = open_notebook(&serde_json::json!({
            "nbformat": 4,
            "metadata": {"language_info": {"name": "python 3!"}},
            "cells": [
                {"cell_type": "markdown", "source": ["# Title\n", "text"]},
                {"cell_type": "code", "source": "print(1)", "outputs": [
                    {"output_type": "stream", "text": ["1\n"]},
                    {"output_type": "display_data", "data": {"image/png": "iVBO\nRw0K", "text/plain": "<Figure>"}},
                    {"output_type": "execute_result", "data": {"text/plain": "   "}}
                ]},
                {"cell_type": "raw", "source": "raw"}
            ]
        }).to_string())
        .unwrap();
        assert_eq!(notebook.language, "python3");
        assert_eq!(notebook.cells[0].source, "# Title\ntext");
        assert_eq!(notebook.cells[1].outputs.len(), 2, "blank outputs are dropped");
        assert_eq!(notebook.cells[1].outputs[1].text, "data:image/png;base64,iVBORw0K");
        assert_eq!(notebook.cells[2].kind, CellKind::Raw);
        assert_eq!(notebook_cell_language("python"), "py");
    }

    #[test]
    fn notebooks_outside_the_limits_are_refused() {
        assert!(open_notebook("{\"nbformat\":3,\"cells\":[]}").is_err());
        assert!(open_notebook("not json").is_err());
        let outputs: Vec<_> = (0..101).map(|_| serde_json::json!({"text": "x"})).collect();
        let many = serde_json::json!({"nbformat": 4, "cells": [{"cell_type": "code", "source": "", "outputs": outputs}]});
        assert_eq!(open_notebook(&many.to_string()), Err("A notebook cell has too many outputs to preview."));
    }
}
