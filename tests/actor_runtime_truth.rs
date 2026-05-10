use std::fs;
use std::path::{Path, PathBuf};

struct SourceFile {
    path: PathBuf,
    content: String,
}

impl SourceFile {
    fn read(path: PathBuf) -> Self {
        let content = fs::read_to_string(&path).expect("source file is readable");
        Self { path, content }
    }

    fn is_guard_source(&self) -> bool {
        self.path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "actor_runtime_truth.rs")
    }

    fn contains(&self, fragment: &str) -> bool {
        self.content.contains(fragment)
    }
}

struct SourceTree {
    root: PathBuf,
}

impl SourceTree {
    fn new() -> Self {
        Self {
            root: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        }
    }

    fn guarded_files(&self) -> Vec<SourceFile> {
        let mut files = vec![
            self.root.join("Cargo.toml"),
            self.root.join("Cargo.lock"),
            self.root.join("src").join("terminal.rs"),
        ];
        files.extend(self.test_files());
        files.into_iter().map(SourceFile::read).collect()
    }

    fn test_files(&self) -> Vec<PathBuf> {
        let tests = self.root.join("tests");
        fs::read_dir(tests)
            .expect("tests directory is readable")
            .map(|entry| entry.expect("test entry is readable").path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
            .collect()
    }
}

#[test]
fn terminal_delivery_actor_cannot_use_non_kameo_runtime() {
    let forbidden_fragments = [
        "ractor =",
        "name = \"ractor\"",
        "use ractor",
        "ractor::",
        "RpcReplyPort",
        "ActorProcessingErr",
    ];

    let mut violations = Vec::new();
    for file in SourceTree::new().guarded_files() {
        if file.is_guard_source() {
            continue;
        }
        for fragment in forbidden_fragments {
            if file.contains(fragment) {
                violations.push(format!("{} contains {fragment}", file.path.display()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "non-kameo terminal actor runtime violations:\n{}",
        violations.join("\n")
    );
}

#[test]
fn terminal_delivery_actor_cannot_be_empty_marker() {
    let terminal_source = SourceFile::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("terminal.rs"),
    );

    assert!(terminal_source.contains("pub struct TerminalDeliveryActor {"));
    assert!(terminal_source.contains("backend: WezTermMux,"));
    assert!(terminal_source.contains("delivered_prompt_count: u64,"));
}
