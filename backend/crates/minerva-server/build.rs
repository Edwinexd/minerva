use std::{env, fs, path::Path};

fn main() {
    let manifest_path = Path::new("src/strategy/global-knowledge.json");
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    let mut manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(manifest_path).expect("read global knowledge manifest"),
    )
    .expect("parse global knowledge manifest");
    for source in manifest["sources"].as_array_mut().expect("sources array") {
        let content_file = source["contentFile"].as_str().expect("contentFile");
        let content_key = source["contentKey"].as_str().expect("contentKey");
        let path = manifest_path.parent().unwrap().join(content_file);
        println!("cargo:rerun-if-changed={}", path.display());
        let content: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).expect("read global knowledge source"))
                .expect("parse global knowledge source");
        let selected = content_key
            .split('.')
            .fold(&content, |value, key| value.get(key).expect("contentKey"));
        source["content"] = selected.clone();
    }
    fs::write(
        Path::new(&env::var("OUT_DIR").unwrap()).join("global-knowledge.resolved.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .expect("write resolved global knowledge manifest");
}
