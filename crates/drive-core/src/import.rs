//! Turning the paths a user picked into the list of files to upload.
//!
//! F4 of `docs/rewrite-plan.md` §8. Only the directory walk lives here: the native file dialog
//! and the window drag-and-drop that produce the paths are Tauri (F5), because `drive-core`
//! must never depend on `tauri`. F5 collects them and hands them to [`expand`] as plain paths.

use std::path::{Path, PathBuf};

use crate::error::{AppError, Result};
use crate::upload_queue::{UploadTask, mime_for};

/// Junk that accumulates in every synced folder and that nobody ever wants uploaded.
const JUNK_FILE_NAMES: [&str; 3] = [".DS_Store", "Thumbs.db", "desktop.ini"];

// shortcut: MAX_DEPTH = 32 directory levels below the picked root, so a pathological tree cannot
// hang the UI. When a real tree reaches it, surface a "folder too deep" signal instead of
// silently truncating.
const MAX_DEPTH: usize = 32;

/// One file to upload, plus where it sat inside the tree the user picked.
#[derive(Debug, PartialEq, Eq)]
pub struct ImportCandidate {
    /// Absolute local path to read from.
    pub source_path: PathBuf,
    /// Path relative to the root the user picked, for recreating the tree as virtual folders.
    /// `None` when the file sat at the top level of what was picked — a file picked directly, or
    /// a child of a picked directory. There is deliberately no empty-`PathBuf` case.
    pub relative_dir: Option<PathBuf>,
    pub size_bytes: u64,
}

/// Expand the paths the user picked into upload candidates.
///
/// Each entry is either a file (expands to itself, `relative_dir == None`) or a directory, which
/// is walked recursively. Symlinks and junk discovered by the walk are skipped; a junk file the
/// user picked by name is not. Missing or unreadable picked paths are [`AppError::InvalidInput`];
/// entries found deeper are skipped instead.
pub fn expand(paths: &[PathBuf]) -> Result<Vec<ImportCandidate>> {
    let mut out = Vec::new();
    for path in paths {
        // A picked path is a direct request, so follow its symlink and fail loudly rather than
        // silently importing nothing.
        let meta = std::fs::metadata(path).map_err(|e| {
            AppError::InvalidInput(format!("cannot read picked path {}: {e}", path.display()))
        })?;
        if meta.is_dir() {
            walk(path, Path::new(""), 0, &mut out);
        } else {
            // No junk filter here, unlike the walk: the user named this file, so dropping it
            // silently would be losing a file they asked for.
            out.push(ImportCandidate {
                source_path: path.clone(),
                relative_dir: None,
                size_bytes: meta.len(),
            });
        }
    }
    // Sorted rather than left in readdir order: two imports of the same tree must enqueue the
    // same files in the same order, or a bug report cannot be reproduced.
    out.sort_by(|a, b| {
        a.relative_dir
            .as_deref()
            .cmp(&b.relative_dir.as_deref())
            .then_with(|| a.source_path.cmp(&b.source_path))
    });
    Ok(out)
}

/// Collect files under `dir`, where `relative` is `dir`'s path inside the picked root.
fn walk(dir: &Path, relative: &Path, depth: usize, out: &mut Vec<ImportCandidate>) {
    if depth > MAX_DEPTH {
        return;
    }
    // A directory unreadable this deep is skipped, not fatal: losing a 10 000-file import to one
    // permission-denied subdirectory is worse than one missing file. That is the opposite of
    // the picked paths, because nobody explicitly asked for what is found deeper.
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        // DirEntry::metadata does not follow symlinks, so a link is spotted without a second
        // syscall: a link out of the picked tree escapes what the user chose, and a link back up
        // would loop the walk forever.
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        let name = entry.file_name();
        let path = dir.join(&name);
        if meta.is_dir() {
            let child = if relative.as_os_str().is_empty() {
                PathBuf::from(&name)
            } else {
                relative.join(&name)
            };
            walk(&path, &child, depth + 1, out);
        } else if !is_junk(&path) {
            out.push(ImportCandidate {
                source_path: path,
                relative_dir: (!relative.as_os_str().is_empty()).then(|| relative.to_path_buf()),
                size_bytes: meta.len(),
            });
        }
    }
}

fn is_junk(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| JUNK_FILE_NAMES.contains(&name))
}

/// A single picked file becomes a task with no id: one file has nothing to be numbered against,
/// and `upload_queue::tasks_for` is what hands out the ids a batch reports and emits under.
impl From<ImportCandidate> for UploadTask {
    fn from(candidate: ImportCandidate) -> Self {
        let source_path = candidate.source_path;
        // The whole file name, extension and all: Drive previews off the extension, so a name
        // stripped to the stem would upload a PDF that no longer looks like one.
        let name = source_path
            .file_name()
            .unwrap_or(source_path.as_os_str())
            .to_string_lossy()
            .into_owned();
        UploadTask {
            task_id: 0,
            mime_type: mime_for(&source_path).to_string(),
            source_path,
            name,
            size_bytes: candidate.size_bytes,
            folder_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Temp directory removed on drop, since a trailing cleanup is skipped by a failed assertion
    /// and would then leak into the next run.
    struct TempTree(PathBuf);

    impl TempTree {
        fn new(label: &str) -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "9drive-import-{}-{label}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("create temp tree");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        /// Writes `rel` inside the tree, creating parents, and returns the absolute path.
        fn write(&self, rel: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create parent dir");
            }
            std::fs::write(&path, bytes).expect("write temp file");
            path
        }

        /// `file_name` of every candidate, for assertions that only care about the names.
        fn names(candidates: &[ImportCandidate]) -> Vec<String> {
            candidates
                .iter()
                .map(|c| {
                    c.source_path
                        .file_name()
                        .expect("candidate name")
                        .to_string_lossy()
                        .into_owned()
                })
                .collect()
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(windows)]
    fn link_file(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_file(target, link)
    }

    #[cfg(windows)]
    fn link_dir(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_dir(target, link)
    }

    #[cfg(unix)]
    fn link_file(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(unix)]
    fn link_dir(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[test]
    fn a_single_picked_file_expands_to_itself() {
        let tree = TempTree::new("single");
        let file = tree.write("notes.txt", b"hello");

        let out = expand(std::slice::from_ref(&file)).expect("expand");

        assert_eq!(
            out,
            vec![ImportCandidate { source_path: file, relative_dir: None, size_bytes: 5 }]
        );
    }

    #[test]
    fn a_picked_directory_expands_its_files_with_relative_dirs() {
        let tree = TempTree::new("dir");
        let top = tree.write("a.txt", b"a");
        let sub = tree.write("sub/b.txt", b"bb");

        let out = expand(&[tree.path().to_path_buf()]).expect("expand");

        assert_eq!(
            out,
            vec![
                ImportCandidate { source_path: top, relative_dir: None, size_bytes: 1 },
                ImportCandidate {
                    source_path: sub,
                    relative_dir: Some(PathBuf::from("sub")),
                    size_bytes: 2,
                },
            ]
        );
    }

    #[test]
    fn a_nested_tree_nests_its_relative_dirs_and_never_uses_an_empty_path() {
        let tree = TempTree::new("nested");
        tree.write("top.txt", b"1");
        tree.write("one/mid.txt", b"22");
        tree.write("one/two/deep.txt", b"333");

        let out = expand(&[tree.path().to_path_buf()]).expect("expand");

        let dirs: Vec<Option<&Path>> = out.iter().map(|c| c.relative_dir.as_deref()).collect();
        assert_eq!(dirs, [None, Some(Path::new("one")), Some(Path::new("one/two"))]);
        assert!(!dirs.iter().any(|dir| dir.is_some_and(|d| d.as_os_str().is_empty())));
    }

    #[test]
    fn two_runs_over_the_same_tree_are_byte_identical() {
        let tree = TempTree::new("stable");
        tree.write("b.txt", b"b");
        tree.write("a/z.txt", b"z");
        tree.write("a/y/x.txt", b"x");
        let root = tree.path().to_path_buf();

        let first = expand(std::slice::from_ref(&root)).expect("first run");
        let second = expand(std::slice::from_ref(&root)).expect("second run");

        assert_eq!(format!("{first:?}"), format!("{second:?}"));
        assert_eq!(first, second);
    }

    #[test]
    fn junk_files_are_skipped_anywhere_in_the_tree() {
        let tree = TempTree::new("junk");
        tree.write("keep.txt", b"k");
        tree.write(".DS_Store", b"j");
        tree.write("sub/Thumbs.db", b"j");
        tree.write("sub/deeper/desktop.ini", b"j");

        let out = expand(&[tree.path().to_path_buf()]).expect("expand");

        assert_eq!(TempTree::names(&out), ["keep.txt"]);
    }

    #[test]
    fn a_junk_file_picked_by_name_is_still_imported() {
        let tree = TempTree::new("picked-junk");
        tree.write(".DS_Store", b"j");

        let out = expand(&[tree.path().join(".DS_Store")]).expect("expand");

        assert_eq!(TempTree::names(&out), [".DS_Store"]);
    }

    #[test]
    fn a_missing_picked_path_is_invalid_input_naming_it() {
        let tree = TempTree::new("missing");
        let ghost = tree.path().join("nope.txt");

        let err = expand(std::slice::from_ref(&ghost)).expect_err("missing path must fail");

        assert!(matches!(err, AppError::InvalidInput(_)), "got {err:?}");
        assert_eq!(err.code(), "invalid_input");
        assert!(err.to_string().contains(&ghost.display().to_string()), "got {err}");
    }

    #[test]
    fn no_picked_paths_yields_nothing() {
        assert_eq!(expand(&[]).expect("expand"), Vec::new());
    }

    #[test]
    fn the_walk_stops_at_max_depth() {
        let tree = TempTree::new("deep");
        tree.write("shallow.txt", b"s");
        tree.write(&format!("{}at_limit.txt", "d/".repeat(MAX_DEPTH)), b"l");
        tree.write(&format!("{}buried.txt", "d/".repeat(MAX_DEPTH + 1)), b"b");

        let out = expand(&[tree.path().to_path_buf()]).expect("expand");

        let mut names = TempTree::names(&out);
        names.sort();
        assert_eq!(names, ["at_limit.txt", "shallow.txt"]);
    }

    #[test]
    fn symlinks_are_skipped_instead_of_followed() {
        let tree = TempTree::new("links");
        let outside = TempTree::new("links-outside");
        outside.write("elsewhere.txt", b"e");
        tree.write("real.txt", b"r");

        let file_link = tree.path().join("link.txt");
        let dir_link = tree.path().join("linkdir");
        if link_file(&outside.path().join("elsewhere.txt"), &file_link).is_err()
            || link_dir(outside.path(), &dir_link).is_err()
        {
            println!("skipped: creating symlinks needs Developer Mode or administrator rights");
            return;
        }

        let out = expand(&[tree.path().to_path_buf()]).expect("expand");

        assert_eq!(TempTree::names(&out), ["real.txt"]);
    }
}
