//! Locating host tools (clang, ld.lld) without any configuration: `PATH`
//! first, then the official LLVM installer's default directory on Windows.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Directories searched after `PATH`.
pub fn default_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if cfg!(windows) {
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            dirs.push(Path::new(&pf).join("LLVM").join("bin"));
        }
        dirs.push(PathBuf::from(r"C:\Program Files\LLVM\bin"));
    }
    dirs
}

/// The file name of an executable on this host (`clang` → `clang.exe` on Windows).
pub fn exe_name(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// Finds `name` on `PATH` or in [`default_dirs`].
pub fn find(name: &str) -> Option<PathBuf> {
    find_in(name, std::env::var_os("PATH").as_deref(), &default_dirs())
}

/// Finds `name` in the directories of a `PATH`-style list, then in `extra`.
pub fn find_in(name: &str, path: Option<&OsStr>, extra: &[PathBuf]) -> Option<PathBuf> {
    let file = exe_name(name);
    path.map(|p| std::env::split_paths(p).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .chain(extra.iter().cloned())
        .filter(|d| !d.as_os_str().is_empty())
        .map(|d| d.join(&file))
        .find(|p| p.is_file())
}

/// Finds `name` next to another tool first (so clang and ld.lld come from the
/// same LLVM install), then like [`find`].
pub fn find_beside(name: &str, sibling: &Path) -> Option<PathBuf> {
    sibling
        .parent()
        .map(|d| d.join(exe_name(name)))
        .filter(|p| p.is_file())
        .or_else(|| find(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("xtask-tools-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn exe_suffix() {
        if cfg!(windows) {
            assert_eq!(exe_name("clang"), "clang.exe");
        } else {
            assert_eq!(exe_name("clang"), "clang");
        }
    }

    #[test]
    fn searches_path_in_order_then_extra() {
        let root = scratch("order");
        let (a, b, c) = (root.join("a"), root.join("b"), root.join("c"));
        for d in [&a, &b, &c] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(b.join(exe_name("tool")), b"").unwrap();
        std::fs::write(c.join(exe_name("tool")), b"").unwrap();
        std::fs::write(c.join(exe_name("other")), b"").unwrap();

        let path = std::env::join_paths([&a, &b]).unwrap();
        let extra = [c.clone()];
        assert_eq!(
            find_in("tool", Some(&path), &extra),
            Some(b.join(exe_name("tool")))
        );
        assert_eq!(
            find_in("other", Some(&path), &extra),
            Some(c.join(exe_name("other")))
        );
        assert_eq!(find_in("missing", Some(&path), &extra), None);
        assert_eq!(find_in("tool", None, &[]), None);
        // A directory with the tool's name is not the tool.
        std::fs::create_dir_all(a.join(exe_name("dir"))).unwrap();
        assert_eq!(find_in("dir", Some(&path), &[]), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn prefers_sibling() {
        let root = scratch("sibling");
        std::fs::write(root.join(exe_name("clang")), b"").unwrap();
        std::fs::write(root.join(exe_name("ld.lld")), b"").unwrap();
        assert_eq!(
            find_beside("ld.lld", &root.join(exe_name("clang"))),
            Some(root.join(exe_name("ld.lld")))
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
