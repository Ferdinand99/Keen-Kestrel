//! Every language translates every one of Kestrel's own strings, with the
//! same placeholders and the plural forms its CLDR rule needs.

use std::collections::BTreeSet;

use vole_kit::i18n::{Lang, Plural, catalog};

const CATEGORIES: [&str; 6] = ["zero", "one", "two", "few", "many", "other"];

fn product_text(code: &str) -> &'static str {
    keen_kestrel::product::KIT
        .locales
        .iter()
        .find(|(c, _)| *c == code)
        .map_or("", |(_, text)| text)
}

/// `(plain keys, plural bases)` of a catalog.
fn split(keys: impl Iterator<Item = String>) -> (BTreeSet<String>, BTreeSet<String>) {
    let (mut plain, mut plural) = (BTreeSet::new(), BTreeSet::new());
    for key in keys {
        match key.rsplit_once('.') {
            Some((base, cat)) if CATEGORIES.contains(&cat) => {
                plural.insert(base.to_string());
            }
            _ => {
                plain.insert(key);
            }
        }
    }
    (plain, plural)
}

fn placeholders(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        rest = &rest[start + 1..];
        if let Some(end) = rest.find('}') {
            out.insert(rest[..end].to_string());
            rest = &rest[end + 1..];
        }
    }
    out
}

#[test]
fn every_language_has_every_string() {
    keen_kestrel::install();
    let en = catalog::parse(product_text("en")).unwrap();
    let (plain, plural) = split(en.keys().cloned());
    assert!(plain.len() > 50 && !plural.is_empty());

    let mut problems = Vec::new();
    let mut languages = 0;
    for lang in Lang::all() {
        let code = lang.code();
        let text = product_text(code);
        if text.is_empty() {
            problems.push(format!("{code}: no catalog"));
            continue;
        }
        languages += 1;
        let cat = catalog::parse(text).unwrap_or_else(|e| panic!("{code}: {e}"));
        let required: Vec<&str> = lang
            .plural_rule()
            .categories()
            .iter()
            .map(|p: &Plural| p.as_str())
            .collect();
        let mut expected: BTreeSet<String> = plain.clone();
        for base in &plural {
            for cat in &required {
                expected.insert(format!("{base}.{cat}"));
            }
        }
        let got: BTreeSet<String> = cat.keys().cloned().collect();
        for missing in expected.difference(&got) {
            problems.push(format!("{code}: missing {missing}"));
        }
        for extra in got.difference(&expected) {
            problems.push(format!("{code}: unexpected {extra}"));
        }
        for (key, value) in &cat {
            if value.trim().is_empty() {
                problems.push(format!("{code}: {key} is empty"));
            }
            let english = en.get(key).or_else(|| {
                key.rsplit_once('.')
                    .and_then(|(base, _)| en.get(&format!("{base}.other")))
            });
            if let Some(english) = english
                && placeholders(english) != placeholders(value)
            {
                problems.push(format!("{code}: {key} has other placeholders than English"));
            }
        }
    }
    assert_eq!(languages, 26, "{problems:#?}");
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// The language picker's languages are exactly the ones Kestrel ships.
#[test]
fn kestrel_ships_every_language_the_kit_knows() {
    let shipped: BTreeSet<&str> = keen_kestrel::product::KIT
        .locales
        .iter()
        .map(|(c, _)| *c)
        .collect();
    let kit: BTreeSet<&str> = Lang::all().map(|l| l.code()).collect();
    assert_eq!(shipped, kit);
}

/// Every key Kestrel's templates and code use exists in its English catalog
/// (or the kit's).
#[test]
fn every_used_key_exists() {
    keen_kestrel::install();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut sources = Vec::new();
    for dir in ["templates", "src"] {
        collect(&root.join(dir), &mut sources);
    }
    let mut missing = Vec::new();
    let mut checked = 0;
    for file in sources {
        if file.components().any(|c| c.as_os_str() == "kit") {
            continue;
        }
        let src = std::fs::read_to_string(&file).unwrap();
        for call in [
            "tr(\"",
            "tr1(\"",
            "trn(\"",
            "i18n::t(\"",
            "i18n::tf(\"",
            "Err(\"",
            "label: \"",
        ] {
            let mut rest = src.as_str();
            while let Some(i) = rest.find(call) {
                let before = rest[..i].chars().next_back();
                rest = &rest[i + call.len()..];
                if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                let Some(end) = rest.find('"') else { break };
                let key = &rest[..end];
                if !key.contains('.') || key.contains(' ') {
                    continue;
                }
                checked += 1;
                let plural = call == "trn(\"";
                let lookup = if plural {
                    format!("{key}.other")
                } else {
                    key.to_string()
                };
                if catalog::get(Lang::EN, &lookup).is_none() {
                    missing.push(format!("{}: {key}", file.display()));
                }
            }
        }
    }
    assert!(checked > 80, "only {checked} keys found");
    assert!(missing.is_empty(), "{}", missing.join("\n"));
}

fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs" || e == "html") {
            out.push(path);
        }
    }
}
