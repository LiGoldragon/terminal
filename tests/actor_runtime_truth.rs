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
            self.root.join("src").join("pty.rs"),
        ];
        files.extend(self.test_files());
        files.into_iter().map(SourceFile::read).collect()
    }

    fn production_files(&self) -> Vec<SourceFile> {
        let mut files = vec![self.root.join("Cargo.toml"), self.root.join("Cargo.lock")];
        files.extend(self.rust_files_under(self.root.join("src")));
        files.into_iter().map(SourceFile::read).collect()
    }

    fn rust_files_under(&self, directory: PathBuf) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let entries = fs::read_dir(directory).expect("source directory is readable");
        for entry in entries {
            let path = entry.expect("source entry is readable").path();
            if path.is_dir() {
                files.extend(self.rust_files_under(path));
                continue;
            }
            if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
        files
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
fn terminal_delivery_cannot_use_non_kameo_runtime() {
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
fn terminal_brand_mux_adapter_is_retired_not_reimplemented() {
    let retired_brand = ["Wez", "Term"].concat();
    let retired_binary = retired_brand.to_ascii_lowercase();
    let retired_socket = ["WEZ", "TERM_UNIX_SOCKET"].concat();
    let forbidden_fragments = [
        "TerminalDelivery",
        "DeliverTerminalPrompt",
        retired_brand.as_str(),
        retired_binary.as_str(),
        retired_socket.as_str(),
    ];

    let mut violations = Vec::new();
    for file in SourceTree::new().production_files() {
        for fragment in forbidden_fragments {
            if file.contains(fragment) {
                violations.push(format!("{} contains {fragment}", file.path.display()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "retired terminal-brand mux adapter fragments:\n{}",
        violations.join("\n")
    );
}

#[test]
fn persona_terminal_uses_terminal_cell_as_the_pty_cell_primitive() {
    let manifest = SourceFile::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"));
    let pty_source = SourceFile::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("pty.rs"),
    );

    assert!(manifest.contains("terminal-cell"));
    assert!(pty_source.contains("TerminalCell::spawn_session"));
    assert!(pty_source.contains("TerminalCellSocketClient"));
}

#[test]
fn terminal_registry_state_goes_through_component_sema() {
    let manifest = SourceFile::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"));
    let tables_source = SourceFile::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("tables.rs"),
    );

    assert!(manifest.contains("sema"));
    assert!(tables_source.contains("Sema::open_with_schema"));
    assert!(tables_source.contains("Table<&'static str, StoredTerminalSession>"));
    assert!(!tables_source.contains("registry.json"));
    assert!(!tables_source.contains("sessions.json"));
}
