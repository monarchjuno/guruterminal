use crate::{knowledge, workspace};
use serde::Serialize;
use std::path::PathBuf;

use guruterminal_core::CanonicalMemoryKind;

pub fn run(arguments: Vec<String>) -> Result<(), String> {
    if arguments.is_empty()
        || matches!(
            arguments.first().map(String::as_str),
            Some("--help") | Some("-h")
        )
    {
        print_help();
        return Ok(());
    }
    if arguments == ["--version"] || arguments == ["version"] {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    match arguments[0].as_str() {
        "init" => init(&arguments[1..]),
        "knowledge" => knowledge_command(&arguments[1..]),
        command => Err(format!("unknown command: {command}")),
    }
}

fn knowledge_command(arguments: &[String]) -> Result<(), String> {
    if arguments.is_empty()
        || matches!(
            arguments.first().map(String::as_str),
            Some("--help") | Some("-h")
        )
    {
        print_knowledge_help();
        return Ok(());
    }
    let (command, rest) = arguments
        .split_first()
        .expect("knowledge arguments were checked above");
    match command.as_str() {
        "list" => knowledge_list(rest),
        "search" => knowledge_search(rest),
        "read" => knowledge_read(rest),
        "check" => knowledge_check(rest),
        "health" => knowledge_health(rest),
        "context" => knowledge_context(rest),
        other => Err(format!("unknown knowledge command: {other}")),
    }
}

fn knowledge_context(arguments: &[String]) -> Result<(), String> {
    let parsed = Options::parse(
        arguments,
        &[
            "json",
            "workspace",
            "check",
            "health",
            "revision",
            "learned-index",
            "charter",
        ],
    )?;
    let usage = "usage: guruterminal-core knowledge context [--check] [--health] [--revision] [--learned-index] [--charter] [--workspace PATH] --json";
    no_positionals(&parsed, usage)?;
    if !parsed.json {
        return Err(format!("{usage}; --json is required"));
    }
    if !["check", "health", "revision", "learned-index", "charter"]
        .iter()
        .any(|name| parsed.has_flag(name))
    {
        return Err(usage.into());
    }
    let root = PathBuf::from(parsed.value("workspace").unwrap_or("."));
    workspace::require_workspace(&root)?;
    let result = knowledge::context(&root)?;

    #[derive(Serialize)]
    struct ContextOutput<'a> {
        #[serde(skip_serializing_if = "Option::is_none")]
        check: Option<&'a knowledge::KnowledgeCheck>,
        #[serde(skip_serializing_if = "Option::is_none")]
        health: Option<&'a knowledge::KnowledgeHealth>,
        #[serde(skip_serializing_if = "Option::is_none")]
        revision: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        records: Option<&'a Vec<knowledge::Document>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        charter: Option<&'a Option<knowledge::KnowledgeCharterRead>>,
    }

    emit(
        &ContextOutput {
            check: parsed.has_flag("check").then_some(&result.check),
            health: parsed.has_flag("health").then_some(&result.health),
            revision: parsed
                .has_flag("revision")
                .then_some(result.revision.as_str()),
            records: parsed.has_flag("learned-index").then_some(&result.records),
            charter: parsed.has_flag("charter").then_some(&result.charter),
        },
        true,
        None,
    )?;
    if parsed.has_flag("check") && !result.check.valid {
        Err("knowledge check failed".into())
    } else {
        Ok(())
    }
}

fn knowledge_read(arguments: &[String]) -> Result<(), String> {
    let parsed = Options::parse(arguments, &["json", "workspace", "section"])?;
    let id = one_positional(
        &parsed,
        "usage: guruterminal-core knowledge read <id> [--section NAME] [--workspace PATH] [--json]",
    )?;
    let root = PathBuf::from(parsed.value("workspace").unwrap_or("."));
    workspace::require_workspace(&root)?;
    let result = knowledge::read(&root, id, parsed.value("section"))?;
    if parsed.json {
        #[derive(Serialize)]
        struct ReadOutput<'a> {
            document: &'a knowledge::Document,
            section: &'a Option<knowledge::Section>,
            content: &'a str,
        }
        emit(
            &ReadOutput {
                document: &result.document,
                section: &result.section,
                content: &result.document.content,
            },
            true,
            None,
        )
    } else if let Some(section) = result.section {
        println!("{}", section.text);
        Ok(())
    } else {
        print!("{}", result.document.content);
        Ok(())
    }
}

fn knowledge_list(arguments: &[String]) -> Result<(), String> {
    let parsed = Options::parse(arguments, &["json", "workspace", "kind"])?;
    no_positionals(
        &parsed,
        "usage: guruterminal-core knowledge list [--kind KIND] [--workspace PATH] [--json]",
    )?;
    let root = PathBuf::from(parsed.value("workspace").unwrap_or("."));
    workspace::require_workspace(&root)?;
    let mut documents = knowledge::catalog_local(&root);
    if let Some(kind) = parsed.value("kind") {
        if CanonicalMemoryKind::from_slug(kind).is_none() {
            return Err(format!("unknown knowledge kind: {kind}"));
        }
        documents.retain(|document| document.kind == kind);
    }
    if parsed.json {
        emit(&documents, true, None)
    } else if documents.is_empty() {
        println!("No Guru Terminal memory records found.");
        Ok(())
    } else {
        for document in documents {
            println!(
                "{}\t{}\t{}\t{}",
                document.kind, document.id, document.as_of, document.title
            );
        }
        Ok(())
    }
}

fn knowledge_search(arguments: &[String]) -> Result<(), String> {
    let parsed = Options::parse_repeatable(
        arguments,
        &[
            "json",
            "workspace",
            "kind",
            "limit",
            "candidates",
            "include-revoked",
            "as-of",
        ],
        &["kind"],
    )?;
    let query = one_positional(&parsed, "usage: guruterminal-core knowledge search <query> [--kind KIND]... [--limit N] [--candidates] [--include-revoked] [--as-of YYYY-MM-DD] [--workspace PATH] [--json]")?;
    let root = PathBuf::from(parsed.value("workspace").unwrap_or("."));
    workspace::require_workspace(&root)?;
    let kinds = parsed.values("kind");
    let limit = parsed
        .value("limit")
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| "--limit must be an integer".to_string())
        })
        .transpose()?
        .unwrap_or(8);
    let as_of = parsed
        .value("as-of")
        .map(|value| {
            chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .map_err(|_| "--as-of must be a YYYY-MM-DD date".to_string())
        })
        .transpose()?;
    if parsed.has_flag("candidates") {
        if as_of.is_some() {
            return Err("--as-of cannot be combined with --candidates".into());
        }
        if parsed.has_flag("include-revoked") {
            return Err("--include-revoked cannot be combined with --candidates".into());
        }
        let results = knowledge::search_candidates_with_kinds(&root, query, &kinds, limit)?;
        if parsed.json {
            return emit(&results, true, None);
        }
        if results.is_empty() {
            println!("No matching Guru Terminal memory records found.");
            return Ok(());
        }
        for result in results {
            println!(
                "{}\t{}\t{}\t{}\t{}",
                result.score,
                result.id,
                result.match_tier.as_str(),
                result.section,
                result.title
            );
            println!("{}", result.summary);
        }
        return Ok(());
    }
    let results = knowledge::search_with_kinds_opts(
        &root,
        query,
        &kinds,
        limit,
        parsed.has_flag("include-revoked"),
        as_of,
    )?;
    if parsed.json {
        emit(&results, true, None)
    } else if results.is_empty() {
        println!("No matching Guru Terminal memory records found.");
        Ok(())
    } else {
        for result in results {
            println!(
                "{}\t{}\t{}\t{}",
                result.score, result.id, result.section, result.as_of
            );
            if !result.relationships.is_empty() {
                println!(
                    "relationships\t{}",
                    result
                        .relationships
                        .iter()
                        .map(|relationship| format!(
                            "{}:{}",
                            relationship.kind, relationship.target
                        ))
                        .collect::<Vec<_>>()
                        .join("\t")
                );
            }
            println!("{}", result.text);
        }
        Ok(())
    }
}

fn knowledge_health(arguments: &[String]) -> Result<(), String> {
    let parsed = Options::parse(arguments, &["json", "workspace", "kind"])?;
    no_positionals(
        &parsed,
        "usage: guruterminal-core knowledge health [--kind KIND] [--workspace PATH] [--json]",
    )?;
    let root = PathBuf::from(parsed.value("workspace").unwrap_or("."));
    workspace::require_workspace(&root)?;
    let result = knowledge::health(&root, parsed.value("kind"))?;
    if parsed.json {
        emit(&result, true, None)
    } else {
        for health in result.kinds {
            println!(
                "{}\t{}\t{}\t{}\t{}",
                health.kind, health.documents, health.folders, health.max_depth, health.review_band
            );
            for advisory in health.advisories {
                println!("advisory\t{}\t{}", advisory.code, advisory.ids.join("\t"));
            }
        }
        Ok(())
    }
}

fn knowledge_check(arguments: &[String]) -> Result<(), String> {
    let parsed = Options::parse(arguments, &["json", "workspace"])?;
    no_positionals(
        &parsed,
        "usage: guruterminal-core knowledge check [--workspace PATH] [--json]",
    )?;
    let root = PathBuf::from(parsed.value("workspace").unwrap_or("."));
    workspace::require_workspace(&root)?;
    let result = knowledge::check(&root);
    if parsed.json {
        emit(&result, true, None)?;
    } else if result.valid {
        println!("Valid Guru Terminal memory: {} documents", result.documents);
    } else {
        for issue in &result.errors {
            eprintln!(
                "guruterminal-core: {}: {}: {}",
                issue.path, issue.field, issue.message
            );
        }
    }
    if result.valid {
        Ok(())
    } else {
        Err("knowledge check failed".into())
    }
}

fn init(arguments: &[String]) -> Result<(), String> {
    let parsed = Options::parse(arguments, &["json"])?;
    if parsed.positionals.len() > 1 {
        return Err("usage: guruterminal-core init [workspace] [--json]".into());
    }
    let workspace = parsed
        .positionals
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let result = workspace::initialize_workspace(&workspace)?;
    emit(
        &result,
        parsed.json,
        Some(format!(
            "Initialized Guru Terminal memory in {}",
            result.root
        )),
    )
}

fn emit<T: Serialize>(value: &T, json: bool, message: Option<String>) -> Result<(), String> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(value).map_err(|error| error.to_string())?
        );
    } else if let Some(message) = message {
        println!("{message}");
    }
    Ok(())
}

struct Options {
    positionals: Vec<String>,
    named: Vec<(String, String)>,
    flags: Vec<String>,
    json: bool,
}

impl Options {
    fn parse(arguments: &[String], allowed: &[&str]) -> Result<Self, String> {
        Self::parse_repeatable(arguments, allowed, &[])
    }

    fn parse_repeatable(
        arguments: &[String],
        allowed: &[&str],
        repeatable: &[&str],
    ) -> Result<Self, String> {
        let mut positionals = Vec::new();
        let mut named = Vec::new();
        let mut flags = Vec::new();
        let mut json = false;
        let mut index = 0;
        while index < arguments.len() {
            let argument = &arguments[index];
            if argument == "--json" {
                if !allowed.contains(&"json") || json {
                    return Err("--json may be supplied only once".into());
                }
                json = true;
            } else if matches!(
                argument.as_str(),
                "--candidates"
                    | "--include-revoked"
                    | "--check"
                    | "--health"
                    | "--revision"
                    | "--learned-index"
                    | "--charter"
            ) {
                let name = argument.trim_start_matches('-');
                if !allowed.contains(&name) || flags.iter().any(|flag| flag == name) {
                    return Err(format!("{argument} may be supplied only once"));
                }
                flags.push(name.into());
            } else if let Some(name) = argument.strip_prefix("--") {
                if !allowed.contains(&name) {
                    return Err(format!("unknown option: {argument}"));
                }
                if !repeatable.contains(&name) && named.iter().any(|(key, _)| key == name) {
                    return Err(format!("{argument} may be supplied only once"));
                }
                index += 1;
                let value = arguments
                    .get(index)
                    .ok_or_else(|| format!("{argument} requires a value"))?;
                if value.starts_with("--") {
                    return Err(format!("{argument} requires a value"));
                }
                named.push((name.into(), value.clone()));
            } else {
                positionals.push(argument.clone());
            }
            index += 1;
        }
        Ok(Self {
            positionals,
            named,
            flags,
            json,
        })
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.named
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
    fn values(&self, name: &str) -> Vec<String> {
        self.named
            .iter()
            .filter(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
            .collect()
    }
    fn has_flag(&self, name: &str) -> bool {
        self.flags.iter().any(|flag| flag == name)
    }
}

fn one_positional<'a>(options: &'a Options, usage: &str) -> Result<&'a str, String> {
    if options.positionals.len() == 1 {
        Ok(&options.positionals[0])
    } else {
        Err(usage.into())
    }
}

fn no_positionals(options: &Options, usage: &str) -> Result<(), String> {
    if options.positionals.is_empty() {
        Ok(())
    } else {
        Err(usage.into())
    }
}

fn print_help() {
    println!(
        "guruterminal-core {}\n\nINTERNAL USE ONLY\n\nUSAGE:\n    guruterminal-core init [workspace] [--json]\n    guruterminal-core knowledge <list|search|read|check|health|context> ...",
        env!("CARGO_PKG_VERSION")
    );
}

fn print_knowledge_help() {
    println!(
        "Knowledge commands:\n    guruterminal-core knowledge search <query> [--kind KIND]... [--limit N] [--candidates] [--include-revoked] [--as-of YYYY-MM-DD] [--workspace PATH] [--json]\n    guruterminal-core knowledge read <id> [--section NAME] [--workspace PATH] [--json]\n    guruterminal-core knowledge list [--kind KIND] [--workspace PATH] [--json]\n    guruterminal-core knowledge check [--workspace PATH] [--json]\n    guruterminal-core knowledge health [--kind KIND] [--workspace PATH] [--json]\n    guruterminal-core knowledge context [--check] [--health] [--revision] [--learned-index] [--charter] [--workspace PATH] --json"
    );
}
