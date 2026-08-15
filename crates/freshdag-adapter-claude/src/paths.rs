//! Path canonicalization for synthesized `fs.*` events.
//!
//! `docs/contracts/execution-ir.md §Filesystem effects`: *"Paths MUST be
//! canonicalized to absolute paths at the emitter; the raw observed path
//! may be included as `raw_path`."*
//!
//! The compile path is pure — it does not touch the filesystem — so this
//! canonicalization is **lexical**: `.` components are dropped, `..`
//! components pop the previous normal component, and relative paths are
//! joined onto the hook payload's `cwd`. Symlinks are NOT resolved;
//! `FsRead::follow_symlink_target` is therefore always `None` on
//! adapter-synthesized events. That gap is declared in the coverage
//! manifest rather than papered over.

use std::path::{Component, Path, PathBuf};

/// Why a path could not be turned into an absolute path.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathResolveError {
    /// The tool input carried a relative path and the hook payload had
    /// no `cwd` to resolve it against.
    #[error("relative path `{path}` cannot be made absolute: hook payload carried no `cwd`")]
    RelativeWithoutCwd {
        /// The relative path as observed.
        path: String,
    },
    /// The tool input carried an empty path.
    #[error("tool input carried an empty path")]
    Empty,
}

/// A resolved path plus the raw observed form, when they differ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPath {
    /// Lexically canonicalized absolute path.
    pub path: PathBuf,
    /// The path as observed, when it differs from `path`.
    pub raw_path: Option<PathBuf>,
}

/// Resolve a tool-input path against the hook payload's `cwd`.
pub fn resolve(raw: &str, cwd: Option<&Path>) -> Result<ResolvedPath, PathResolveError> {
    if raw.is_empty() {
        return Err(PathResolveError::Empty);
    }
    let observed = Path::new(raw);
    let joined = if observed.is_absolute() {
        observed.to_path_buf()
    } else {
        let cwd = cwd.ok_or_else(|| PathResolveError::RelativeWithoutCwd {
            path: raw.to_string(),
        })?;
        cwd.join(observed)
    };
    let normalized = lexically_normalize(&joined);
    let raw_path = if normalized == observed {
        None
    } else {
        Some(observed.to_path_buf())
    };
    Ok(ResolvedPath {
        path: normalized,
        raw_path,
    })
}

/// Drop `.` components and pop on `..`, without consulting the
/// filesystem.
///
/// A leading run of `..` in a relative path is preserved (there is
/// nothing to pop); callers only reach this with absolute inputs.
#[must_use]
pub fn lexically_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    let mut popped_depth: usize = 0;
    let mut rooted = false;
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if popped_depth > 0 && out.pop() {
                    popped_depth -= 1;
                } else if !rooted {
                    // Only a *relative* path can retain a leading `..`;
                    // the parent of the root is the root.
                    out.push("..");
                }
            }
            Component::Normal(_) => {
                out.push(component.as_os_str());
                popped_depth += 1;
            }
            Component::RootDir | Component::Prefix(_) => {
                rooted = true;
                out.push(component.as_os_str());
            }
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_paths_pass_through() {
        let r = resolve("/a/b/c.txt", None).unwrap();
        assert_eq!(r.path, PathBuf::from("/a/b/c.txt"));
        assert_eq!(r.raw_path, None);
    }

    #[test]
    fn relative_paths_join_cwd_and_keep_the_raw_form() {
        let r = resolve("src/lib.rs", Some(Path::new("/repo"))).unwrap();
        assert_eq!(r.path, PathBuf::from("/repo/src/lib.rs"));
        assert_eq!(r.raw_path, Some(PathBuf::from("src/lib.rs")));
    }

    #[test]
    fn dot_and_dotdot_are_normalized_lexically() {
        let r = resolve("/repo/./a/../b/c.txt", None).unwrap();
        assert_eq!(r.path, PathBuf::from("/repo/b/c.txt"));
        assert_eq!(r.raw_path, Some(PathBuf::from("/repo/./a/../b/c.txt")));
    }

    #[test]
    fn dotdot_cannot_escape_the_root() {
        assert_eq!(
            lexically_normalize(Path::new("/../../a")),
            PathBuf::from("/a")
        );
    }

    #[test]
    fn relative_without_cwd_is_an_error_not_a_guess() {
        let err = resolve("src/lib.rs", None).unwrap_err();
        assert!(matches!(err, PathResolveError::RelativeWithoutCwd { .. }));
    }

    #[test]
    fn empty_paths_are_rejected() {
        assert_eq!(
            resolve("", Some(Path::new("/x"))),
            Err(PathResolveError::Empty)
        );
    }
}
