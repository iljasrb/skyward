use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let source = root.join("locales/en.json");
    println!("cargo:rerun-if-changed={}", source.display());
    let json: BTreeMap<String, String> = serde_json::from_str(&fs::read_to_string(&source).expect("read locales/en.json")).expect("parse locales/en.json");
    let mut out = String::from("// Generated from locales/en.json. Do not edit by hand.\n\n");
    for (key, value) in json {
        let name = if key.starts_with(|c: char| c.is_ascii_digit()) {
            format!("S_K_{}", key.to_uppercase())
        } else {
            format!("S_{}", key.to_uppercase())
        };
        out.push_str(&format!("pub const {name}: &str = {value:?};\n"));
    }
    let dest = Path::new(&env::var("OUT_DIR").expect("OUT_DIR")).join("strings.rs");
    fs::write(dest, out).expect("write strings.rs");
}
