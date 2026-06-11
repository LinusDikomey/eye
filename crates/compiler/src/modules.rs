use std::path::{Path, PathBuf};

pub fn module_and_children(
    path: &Path,
    is_root: bool,
) -> (PathBuf, impl use<> + Iterator<Item = (String, PathBuf)>) {
    let (file, modules) = if !path.is_dir() {
        return (path.to_owned(), None.into_iter().flatten());
    } else {
        let file = if is_root {
            path.join("main.eye")
        } else {
            path.join("mod.eye")
        };
        // gather child modules, insert them into the definitions and create Modules for them
        let iter = path
            .read_dir()
            .expect("couldn't read module directory")
            .filter_map(|entry| {
                let entry = entry.expect("failed to read module directory entry");
                let ty = entry
                    .file_type()
                    .expect("failed to access file type of module directory entry");
                let path = entry.path();
                if ty.is_file() {
                    if path.extension().is_some_and(|ext| ext == "eye") {
                        let name = path
                            .with_extension("")
                            .file_name()
                            .and_then(|name| name.to_str())
                            .expect("file doesn't have a valid name")
                            .to_owned();
                        if !matches!(name.as_str(), "main" | "mod") {
                            return Some((name, path));
                        }
                    }
                } else if ty.is_dir() {
                    let path = entry.path();
                    if path.join("mod.eye").exists() {
                        let name = path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .expect("directory doesn't have a valid name")
                            .to_owned();
                        return Some((name, path));
                    }
                }
                None
            });
        (file, Some(iter).into_iter().flatten())
    };
    if !(file.exists() && file.is_file()) {
        panic!("File {} doesn't exist", file.display());
        // return Err(MainError::NoMainFileInProjectDirectory);
    };
    (file, modules)
}

pub fn all_project_files_from_root(path: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    all_project_files_rec(path, true, &mut files);
    files
}

fn all_project_files_rec(path: &Path, is_root: bool, files: &mut Vec<PathBuf>) {
    let (file, children) = module_and_children(path, is_root);
    files.push(file);
    for (_, child_path) in children {
        all_project_files_rec(&child_path, false, files);
    }
}
