use std::collections::{BTreeMap, BTreeSet};

const SCALAR_FIELDS: &[&str] = &[
    "id",
    "title",
    "summary",
    "as_of",
    "source",
    "period",
    "status",
    "revoked_by",
];
const LIST_FIELDS: &[&str] = &[
    "entities",
    "aliases",
    "tags",
    "see_also",
    "uses",
    "supports",
    "contradicts",
    "updates",
];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Frontmatter {
    pub(super) scalar: BTreeMap<String, String>,
    pub(super) lists: BTreeMap<String, Vec<String>>,
    pub(super) declared_fields: BTreeSet<String>,
    pub(super) duplicate_fields: BTreeSet<String>,
}

pub(super) struct ParsedFrontmatter {
    pub(super) metadata: Frontmatter,
    pub(super) body: String,
    pub(super) error: Option<String>,
}

pub(super) fn parse_frontmatter(markdown: &str) -> ParsedFrontmatter {
    let mut lines = markdown.lines();
    if lines.next().map(str::trim) != Some("---") {
        return malformed("frontmatter must begin and end with ---");
    }

    let mut metadata = Frontmatter::default();
    let mut current_list: Option<String> = None;
    let mut parse_error = None;
    let mut closed = false;

    for line in lines.by_ref() {
        if line.trim() == "---" {
            closed = true;
            break;
        }
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }

        if line.chars().next().is_some_and(char::is_whitespace) {
            match current_list.as_ref() {
                Some(key) => match line.trim().strip_prefix("- ") {
                    Some(value) if !clean(value).is_empty() => metadata
                        .lists
                        .entry(key.clone())
                        .or_default()
                        .push(clean(value)),
                    _ => {
                        parse_error.get_or_insert_with(|| {
                            "frontmatter list items must use '- value' syntax".to_owned()
                        });
                    }
                },
                None => {
                    parse_error.get_or_insert_with(|| {
                        "frontmatter contains unexpected indented content".to_owned()
                    });
                }
            };
            continue;
        }

        let Some((raw_key, raw_value)) = line.split_once(':') else {
            parse_error.get_or_insert_with(|| {
                "frontmatter fields must use 'name: value' syntax".to_owned()
            });
            current_list = None;
            continue;
        };
        let key = raw_key.trim();
        let value = raw_value.trim();
        if key.is_empty() || (!SCALAR_FIELDS.contains(&key) && !LIST_FIELDS.contains(&key)) {
            parse_error.get_or_insert_with(|| format!("unknown frontmatter field: {key}"));
            current_list = None;
            continue;
        }
        if !metadata.declared_fields.insert(key.to_owned()) {
            metadata.duplicate_fields.insert(key.to_owned());
        }
        if LIST_FIELDS.contains(&key) {
            current_list = value.is_empty().then(|| key.to_owned());
            metadata.lists.insert(key.to_owned(), inline_list(value));
        } else {
            current_list = None;
            metadata.scalar.insert(key.to_owned(), clean(value));
        }
    }

    if !closed {
        return malformed("frontmatter must begin and end with ---");
    }

    ParsedFrontmatter {
        metadata,
        body: lines.collect::<Vec<_>>().join("\n"),
        error: parse_error,
    }
}

fn malformed(message: &str) -> ParsedFrontmatter {
    ParsedFrontmatter {
        metadata: Frontmatter::default(),
        body: String::new(),
        error: Some(message.to_owned()),
    }
}

fn inline_list(value: &str) -> Vec<String> {
    let value = clean(value);
    if value == "[]" {
        Vec::new()
    } else if value.starts_with('[') && value.ends_with(']') {
        value[1..value.len() - 1]
            .split(',')
            .map(clean)
            .filter(|item| !item.is_empty())
            .collect()
    } else if value.is_empty() {
        Vec::new()
    } else {
        vec![value]
    }
}

fn clean(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        value[1..value.len() - 1].trim().to_owned()
    } else {
        value.to_owned()
    }
}
