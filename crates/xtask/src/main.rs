//! `cargo xtask check-deps`: enforces the crate dependency rules from plan
//! §4.2, so an accidental `use` doesn't quietly turn the layered
//! architecture into a ball of mud.
//!
//! Each entry below is a crate's *directory* name under `crates/` mapped to
//! the directory names of the internal crates it's allowed to depend on.
//! Anything not listed here (third-party crates, `xtask` itself) is
//! ignored — this only polices crate-to-crate edges inside the workspace.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use anyhow::{Context, Result, bail};

/// Maps a crate's Cargo.toml package name back to its `crates/<dir>` name,
/// since the two aren't always identical (`archroom-ui` lives in
/// `ui-kit`, `libraw-sys` has no `archroom-` prefix).
fn package_to_dir(package: &str) -> &str {
    match package {
        "archroom-ui" => "ui-kit",
        "libraw-sys" => "libraw-sys",
        other => other.strip_prefix("archroom-").unwrap_or(other),
    }
}

fn allowed_rules() -> HashMap<&'static str, BTreeSet<&'static str>> {
    let mut rules: HashMap<&'static str, BTreeSet<&'static str>> = HashMap::new();
    let below_ui = [
        "core", "color", "io", "catalog", "jobs", "engine", "preview", "services",
    ];

    rules.insert("core", [].into_iter().collect());
    rules.insert("color", ["core"].into_iter().collect());
    rules.insert("libraw-sys", [].into_iter().collect());
    rules.insert("io", ["core", "color", "libraw-sys"].into_iter().collect());
    rules.insert("catalog", ["core"].into_iter().collect());
    rules.insert("jobs", ["core"].into_iter().collect());
    rules.insert("engine", ["core", "color", "io"].into_iter().collect());
    rules.insert("preview", below_ui.into_iter().collect());
    rules.insert("services", below_ui.into_iter().collect());
    rules.insert("ui-kit", ["core"].into_iter().collect());
    // Not in the plan's original table (see crates/shell's module doc
    // comment): the shared `Module`/`AppCx` contract that both module
    // crates and `app` depend on, without a cycle back through them.
    rules.insert(
        "shell",
        ["core", "catalog", "jobs", "services", "ui-kit"]
            .into_iter()
            .collect(),
    );
    rules.insert(
        "module-library",
        ["core", "services", "ui-kit", "shell"]
            .into_iter()
            .collect(),
    );
    rules.insert(
        "module-develop",
        ["core", "services", "ui-kit", "shell"]
            .into_iter()
            .collect(),
    );
    rules.insert(
        "app",
        [
            "core",
            "catalog",
            "jobs",
            "services",
            "ui-kit",
            "shell",
            "module-library",
            "module-develop",
        ]
        .into_iter()
        .collect(),
    );
    let mut cli_allowed = below_ui.to_vec();
    cli_allowed.push("libraw-sys");
    rules.insert("cli", cli_allowed.into_iter().collect());

    rules
}

fn internal_deps_of(cargo_toml_path: &Path) -> Result<Vec<String>> {
    let text = std::fs::read_to_string(cargo_toml_path)
        .with_context(|| format!("reading {}", cargo_toml_path.display()))?;
    let doc: toml::Value = text
        .parse()
        .with_context(|| format!("parsing {}", cargo_toml_path.display()))?;

    let mut deps = Vec::new();
    for table_name in ["dependencies", "dev-dependencies", "build-dependencies"] {
        let Some(table) = doc.get(table_name).and_then(|v| v.as_table()) else {
            continue;
        };
        for key in table.keys() {
            if key.starts_with("archroom-") || key == "libraw-sys" {
                deps.push(package_to_dir(key).to_string());
            }
        }
    }
    Ok(deps)
}

fn check_deps(workspace_root: &Path) -> Result<()> {
    let rules = allowed_rules();
    let crates_dir = workspace_root.join("crates");
    let mut violations = Vec::new();

    let mut dirs: Vec<_> = std::fs::read_dir(&crates_dir)
        .with_context(|| format!("reading {}", crates_dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();

    for dir in dirs {
        let dir_name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if dir_name == "xtask" {
            continue; // build tooling, not part of the layered architecture
        }
        let Some(allowed) = rules.get(dir_name.as_str()) else {
            violations.push(format!(
                "{dir_name}: no dependency rule defined in xtask — add one"
            ));
            continue;
        };

        let cargo_toml = dir.join("Cargo.toml");
        if !cargo_toml.exists() {
            continue;
        }
        let deps = internal_deps_of(&cargo_toml)?;
        for dep in deps {
            if dep == dir_name {
                continue;
            }
            if !allowed.contains(dep.as_str()) {
                violations.push(format!(
                    "{dir_name} depends on {dep}, which plan §4.2 doesn't allow (allowed: {allowed:?})"
                ));
            }
        }
    }

    if violations.is_empty() {
        println!("dependency rules ok ({} crates checked)", rules.len());
        Ok(())
    } else {
        for v in &violations {
            eprintln!("violation: {v}");
        }
        bail!("{} dependency rule violation(s)", violations.len());
    }
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_default();

    // `cargo xtask <cmd>` runs with CWD at the workspace root.
    let workspace_root = std::env::current_dir()?;

    match cmd.as_str() {
        "check-deps" => check_deps(&workspace_root),
        _ => {
            eprintln!("usage: cargo xtask check-deps");
            std::process::exit(2);
        }
    }
}
