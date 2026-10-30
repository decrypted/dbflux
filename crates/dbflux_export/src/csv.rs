use crate::ExportError;
use csv::{Writer, WriterBuilder};
use dbflux_core::{QueryResult, QueryResultShape, Value};
use std::io::Write;

/// The field separator of a CSV document. Semicolon is what spreadsheets in
/// locales with a decimal comma expect; tab gives TSV.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CsvDelimiter {
    #[default]
    Comma,
    Semicolon,
    Tab,
}

impl CsvDelimiter {
    pub const ALL: [CsvDelimiter; 3] = [Self::Comma, Self::Semicolon, Self::Tab];

    pub fn byte(self) -> u8 {
        match self {
            Self::Comma => b',',
            Self::Semicolon => b';',
            Self::Tab => b'\t',
        }
    }
}

/// How a CSV document is written. The default is what every CSV export
/// wrote before the options existed: a header row and commas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CsvOptions {
    pub include_header: bool,
    pub delimiter: CsvDelimiter,
}

impl Default for CsvOptions {
    fn default() -> Self {
        Self {
            include_header: true,
            delimiter: CsvDelimiter::Comma,
        }
    }
}

impl CsvOptions {
    pub(crate) fn writer<W: Write>(self, writer: W) -> Writer<W> {
        WriterBuilder::new()
            .delimiter(self.delimiter.byte())
            .from_writer(writer)
    }
}

pub struct CsvExporter;

impl CsvExporter {
    pub fn export(&self, result: &QueryResult, writer: &mut dyn Write) -> Result<(), ExportError> {
        self.export_with_options(result, CsvOptions::default(), writer)
    }

    pub fn export_with_options(
        &self,
        result: &QueryResult,
        options: CsvOptions,
        writer: &mut dyn Write,
    ) -> Result<(), ExportError> {
        match &result.shape {
            QueryResultShape::Text => {
                if let Some(body) = &result.text_body {
                    writer.write_all(body.as_bytes())?;
                }
                return Ok(());
            }
            QueryResultShape::Binary => {
                if let Some(bytes) = &result.raw_bytes {
                    writer.write_all(bytes)?;
                }
                return Ok(());
            }
            QueryResultShape::Table | QueryResultShape::Json => {}
        }

        let mut csv_writer = options.writer(writer);

        if options.include_header {
            let headers: Vec<&str> = result.columns.iter().map(|c| c.name.as_str()).collect();
            csv_writer.write_record(&headers)?;
        }

        for row in &result.rows {
            for value in row.iter() {
                let field = value_to_csv_field(value);
                csv_writer.write_field(&field)?;
            }
            csv_writer.write_record(None::<&[u8]>)?;
        }

        csv_writer.flush()?;
        Ok(())
    }
}

/// A value as one CSV field writes it, before quoting: empty for NULL, bytes
/// as `\x` hex, dates in ISO form, arrays and documents as JSON.
pub fn value_to_csv_field(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(b) => if *b { "true" } else { "false" }.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => {
            if f.is_nan() {
                "NaN".to_string()
            } else if f.is_infinite() {
                if f.is_sign_positive() {
                    "Infinity".to_string()
                } else {
                    "-Infinity".to_string()
                }
            } else {
                f.to_string()
            }
        }
        Value::Text(s) | Value::Json(s) | Value::Decimal(s) => s.clone(),
        Value::Bytes(b) => format!("\\x{}", hex::encode(b)),
        Value::DateTime(dt) => dt.to_rfc3339(),
        Value::Date(d) => d.format("%Y-%m-%d").to_string(),
        Value::Time(t) => t.format("%H:%M:%S%.f").to_string(),
        Value::ObjectId(id) => id.clone(),
        Value::Unsupported(type_name) => format!("UNSUPPORTED<{}>", type_name),
        Value::Array(arr) => serde_json::to_string(arr).unwrap_or_else(|_| "[]".to_string()),
        Value::Document(doc) => serde_json::to_string(doc).unwrap_or_else(|_| "{}".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbflux_core::ColumnMeta;
    use std::time::Duration;

    fn make_result(columns: Vec<&str>, rows: Vec<Vec<Value>>) -> QueryResult {
        QueryResult::table(
            columns
                .into_iter()
                .map(|name| ColumnMeta {
                    name: name.to_string(),
                    type_name: "text".to_string(),
                    kind: dbflux_core::ColumnKind::Unknown,
                    nullable: true,
                    is_primary_key: false,
                })
                .collect(),
            rows,
            None,
            Duration::from_millis(10),
        )
    }

    #[test]
    fn exports_simple_data() {
        let result = make_result(
            vec!["id", "name"],
            vec![
                vec![Value::Int(1), Value::Text("Alice".to_string())],
                vec![Value::Int(2), Value::Text("Bob".to_string())],
            ],
        );

        let mut buf = Vec::new();
        CsvExporter.export(&result, &mut buf).unwrap();

        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("id,name"));
        assert!(output.contains("1,Alice"));
        assert!(output.contains("2,Bob"));
    }

    #[test]
    fn handles_commas_and_quotes() {
        let result = make_result(
            vec!["text"],
            vec![
                vec![Value::Text("hello, world".to_string())],
                vec![Value::Text("say \"hello\"".to_string())],
            ],
        );

        let mut buf = Vec::new();
        CsvExporter.export(&result, &mut buf).unwrap();

        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("\"hello, world\""));
        assert!(output.contains("\"say \"\"hello\"\"\""));
    }

    #[test]
    fn handles_newlines() {
        let result = make_result(
            vec!["text"],
            vec![vec![Value::Text("line1\nline2".to_string())]],
        );

        let mut buf = Vec::new();
        CsvExporter.export(&result, &mut buf).unwrap();

        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("\"line1\nline2\""));
    }

    #[test]
    fn null_exports_as_empty_field() {
        let result = make_result(
            vec!["a", "b"],
            vec![vec![Value::Text("x".to_string()), Value::Null]],
        );

        let mut buf = Vec::new();
        CsvExporter.export(&result, &mut buf).unwrap();

        let output = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines, vec!["a,b", "x,"]);
        assert!(!output.contains("\\N"));
    }

    #[test]
    fn null_and_empty_string_share_representation() {
        let result = make_result(
            vec!["null_col", "empty_col"],
            vec![vec![Value::Null, Value::Text(String::new())]],
        );

        let mut buf = Vec::new();
        CsvExporter.export(&result, &mut buf).unwrap();

        let output = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1], ",");
    }

    #[test]
    fn handles_all_value_types() {
        let result = make_result(
            vec!["bool", "int", "float", "text", "bytes"],
            vec![vec![
                Value::Bool(true),
                Value::Int(42),
                Value::Float(2.5),
                Value::Text("hello".to_string()),
                Value::Bytes(vec![0xDE, 0xAD, 0xBE, 0xEF]),
            ]],
        );

        let mut buf = Vec::new();
        CsvExporter.export(&result, &mut buf).unwrap();

        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("true"));
        assert!(output.contains("42"));
        assert!(output.contains("2.5"));
        assert!(output.contains("hello"));
        assert!(output.contains("\\xdeadbeef"));
    }

    #[test]
    fn nan_exports_as_nan_string() {
        let result = make_result(vec!["value"], vec![vec![Value::Float(f64::NAN)]]);

        let mut buf = Vec::new();
        CsvExporter.export(&result, &mut buf).unwrap();

        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("NaN"));
    }

    #[test]
    fn handles_infinity() {
        let result = make_result(
            vec!["pos_inf", "neg_inf"],
            vec![vec![
                Value::Float(f64::INFINITY),
                Value::Float(f64::NEG_INFINITY),
            ]],
        );

        let mut buf = Vec::new();
        CsvExporter.export(&result, &mut buf).unwrap();

        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("Infinity"));
        assert!(output.contains("-Infinity"));
    }

    #[test]
    fn handles_empty_result() {
        let result = make_result(vec!["id", "name"], vec![]);

        let mut buf = Vec::new();
        CsvExporter.export(&result, &mut buf).unwrap();

        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output.trim(), "id,name");
    }

    #[test]
    fn without_header_writes_only_the_rows() {
        let result = make_result(
            vec!["id", "name"],
            vec![
                vec![Value::Int(1), Value::Text("Alice".to_string())],
                vec![Value::Int(2), Value::Text("Bob".to_string())],
            ],
        );
        let options = CsvOptions {
            include_header: false,
            ..CsvOptions::default()
        };

        let mut buf = Vec::new();
        CsvExporter
            .export_with_options(&result, options, &mut buf)
            .unwrap();

        let output = String::from_utf8(buf).unwrap();
        assert_eq!(output.lines().collect::<Vec<_>>(), vec!["1,Alice", "2,Bob"]);
    }

    #[test]
    fn without_header_an_empty_result_writes_nothing() {
        let result = make_result(vec!["id", "name"], vec![]);
        let options = CsvOptions {
            include_header: false,
            ..CsvOptions::default()
        };

        let mut buf = Vec::new();
        CsvExporter
            .export_with_options(&result, options, &mut buf)
            .unwrap();

        assert!(buf.is_empty());
    }

    #[test]
    fn semicolon_and_tab_delimiters_separate_and_quote_by_that_byte() {
        let result = make_result(
            vec!["id", "note"],
            vec![vec![Value::Int(1), Value::Text("a;b, c".to_string())]],
        );

        let mut semicolon = Vec::new();
        CsvExporter
            .export_with_options(
                &result,
                CsvOptions {
                    delimiter: CsvDelimiter::Semicolon,
                    ..CsvOptions::default()
                },
                &mut semicolon,
            )
            .unwrap();
        let semicolon = String::from_utf8(semicolon).unwrap();
        assert_eq!(
            semicolon.lines().collect::<Vec<_>>(),
            vec!["id;note", "1;\"a;b, c\""]
        );

        let mut tab = Vec::new();
        CsvExporter
            .export_with_options(
                &result,
                CsvOptions {
                    delimiter: CsvDelimiter::Tab,
                    ..CsvOptions::default()
                },
                &mut tab,
            )
            .unwrap();
        let tab = String::from_utf8(tab).unwrap();
        assert_eq!(
            tab.lines().collect::<Vec<_>>(),
            vec!["id\tnote", "1\ta;b, c"]
        );
    }

    #[test]
    fn default_options_match_the_plain_export() {
        let result = make_result(vec!["a"], vec![vec![Value::Text("x".to_string())]]);

        let mut plain = Vec::new();
        CsvExporter.export(&result, &mut plain).unwrap();
        let mut with_defaults = Vec::new();
        CsvExporter
            .export_with_options(&result, CsvOptions::default(), &mut with_defaults)
            .unwrap();

        assert_eq!(plain, with_defaults);
    }

    #[test]
    fn large_binary_exports_efficiently() {
        let large_blob = vec![0xAB; 10000];
        let result = make_result(vec!["data"], vec![vec![Value::Bytes(large_blob)]]);

        let mut buf = Vec::new();
        CsvExporter.export(&result, &mut buf).unwrap();

        let output = String::from_utf8(buf).unwrap();
        assert!(output.contains("\\x"));
        assert!(output.contains(&"ab".repeat(10000)));
    }
}
