//! Every language translates every one of Kestrel's own strings, with the
//! same placeholders and the plural forms its CLDR rule needs; every key the
//! code and templates use exists.

use std::collections::BTreeSet;

use vole_kit::i18n::{Lang, catalog};

/// Every language Kestrel ships is complete over the joined catalog (the
/// kit's strings and Kestrel's, the texts the kit takes from Kestrel
/// included): no missing or extra keys, no empty values, the same
/// placeholders as English and the plural forms the language needs.
#[test]
fn every_language_is_complete() {
    keen_kestrel::install();
    let en = catalog::parse(
        keen_kestrel::product::KIT
            .locales
            .iter()
            .find(|(c, _)| *c == "en")
            .map_or("", |(_, text)| text),
    )
    .unwrap();
    assert!(en.len() > 100, "only {} English strings", en.len());
    catalog::assert_full_coverage();
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
