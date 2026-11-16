//! Record titles from a template (ADR 0038): `"{name} {grade}"`.

use serde_json::Value;

use crate::Data;

/// A parsed title template: text with `{field}` placeholders.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TitleTemplate {
    parts: Vec<Part>,
}

#[derive(Debug, Clone, PartialEq)]
enum Part {
    Text(String),
    Field(String),
}

impl TitleTemplate {
    /// The template of a single field.
    #[cfg(test)]
    pub(crate) fn field(name: &str) -> Self {
        Self { parts: vec![Part::Field(name.to_owned())] }
    }

    /// Parses `template`; an error names what is wrong: an unclosed or stray
    /// brace, an empty placeholder, or no placeholder at all.
    pub(crate) fn parse(template: &str) -> Result<Self, String> {
        let mut parts = Vec::new();
        let mut rest = template;
        while let Some(start) = rest.find(['{', '}']) {
            if rest[start..].starts_with('}') {
                return Err("has a `}` without its `{`".into());
            }
            if start > 0 {
                parts.push(Part::Text(rest[..start].to_owned()));
            }
            let after = &rest[start + 1..];
            let end = after.find(['{', '}']).filter(|&i| after[i..].starts_with('}')).ok_or("has an unclosed `{`")?;
            let name = after[..end].trim();
            if name.is_empty() {
                return Err("has an empty `{}`".into());
            }
            parts.push(Part::Field(name.to_owned()));
            rest = &after[end + 1..];
        }
        if !rest.is_empty() {
            parts.push(Part::Text(rest.to_owned()));
        }
        if !parts.iter().any(|p| matches!(p, Part::Field(_))) {
            return Err("names no field (write fields as `{name}`)".into());
        }
        Ok(Self { parts })
    }

    /// The fields of the template, in order, possibly repeated.
    pub(crate) fn fields(&self) -> impl Iterator<Item = &str> {
        self.parts.iter().filter_map(|p| match p {
            Part::Field(name) => Some(name.as_str()),
            Part::Text(_) => None,
        })
    }

    /// The single field of a template that holds exactly one (with or without text).
    pub(crate) fn single_field(&self) -> Option<&str> {
        let mut fields = self.fields();
        let first = fields.next()?;
        fields.all(|f| f == first).then_some(first)
    }

    /// The title of `record`: each field's value as text (`null` empty), runs
    /// of spaces collapsed, trimmed; `None` when nothing is left.
    pub(crate) fn render(&self, record: &Data) -> Option<String> {
        let mut out = String::new();
        for part in &self.parts {
            match part {
                Part::Text(text) => out.push_str(text),
                Part::Field(name) => match record.get(name) {
                    None | Some(Value::Null) => {}
                    Some(Value::String(text)) => out.push_str(text),
                    Some(value) => out.push_str(&value.to_string()),
                },
            }
        }
        let title = out.split_whitespace().collect::<Vec<_>>().join(" ");
        (!title.is_empty()).then_some(title)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn record(value: Value) -> Data {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn renders_fields_and_text() {
        let template = TitleTemplate::parse("{name} (grade {grade})").unwrap();
        assert_eq!(template.fields().collect::<Vec<_>>(), ["name", "grade"]);
        assert_eq!(template.render(&record(json!({"name": "Math", "grade": 10}))).as_deref(), Some("Math (grade 10)"));
        assert_eq!(template.render(&record(json!({"name": "Math", "grade": null}))).as_deref(), Some("Math (grade )"));
        assert_eq!(
            TitleTemplate::parse("{a} {b}").unwrap().render(&record(json!({"a": " x ", "b": null}))).as_deref(),
            Some("x")
        );
        assert_eq!(TitleTemplate::field("a").render(&record(json!({"a": ""}))), None, "empty: no title");
        assert_eq!(TitleTemplate::parse("#{ id }").unwrap().single_field(), Some("id"));
        assert_eq!(TitleTemplate::parse("{a} {b}").unwrap().single_field(), None);
    }

    #[test]
    fn refuses_malformed_templates() {
        assert_eq!(TitleTemplate::parse("{name"), Err("has an unclosed `{`".into()));
        assert_eq!(TitleTemplate::parse("{a{b}}"), Err("has an unclosed `{`".into()));
        assert_eq!(TitleTemplate::parse("name}"), Err("has a `}` without its `{`".into()));
        assert_eq!(TitleTemplate::parse("{ }"), Err("has an empty `{}`".into()));
        assert_eq!(TitleTemplate::parse("plain"), Err("names no field (write fields as `{name}`)".into()));
    }
}
