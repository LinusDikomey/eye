use color_format::{ceprintln, cwriteln};
use compiler::compiler::ModuleStorage;

use std::{fmt::Write, path::Path};

use crate::{MainError, path_arg};

pub(crate) fn format(
    path: Option<String>,
    fmt_args: crate::args::FmtArgs,
) -> Result<(), MainError> {
    let (name, storage) = path_arg(path)?;
    eprintln!("Formatting project {name}");
    let mut had_errors = false;
    let mut ok = true;
    let path = match storage {
        ModuleStorage::Path(path) => path,
        ModuleStorage::String(contents) => {
            let (formatted, errors) = format::format(contents);
            if errors.error_count() > 0 {
                // TODO: print errors here
                return Err(MainError::ErrorsFound);
            }
            print!("{formatted}");
            return Ok(());
        }
    };
    for file in compiler::all_project_files_from_root(&path) {
        let src = std::fs::read_to_string(&file).unwrap_or_else(|err| {
            panic!("Failed to read project file {}: {err:?}", file.display())
        });
        let (formatted, errors) = format::format(src.clone().into());
        if errors.error_count() > 0 {
            // TODO: print errors here
            had_errors = true;
            continue;
        }
        if fmt_args.check {
            ok &= check(&file, &src, &formatted);
        } else {
            std::fs::write(&file, formatted).unwrap_or_else(|err| {
                panic!(
                    "Failed to write project file {} after formatting: {err:?}",
                    file.display()
                )
            })
        }
    }
    if had_errors {
        return Err(MainError::ErrorsFound);
    }
    if !ok {
        return Err(MainError::ErrorsFound);
    }
    Ok(())
}

fn check(file: &Path, a: &str, b: &str) -> bool {
    let diff = diff::lines(a, b);
    let mut i = 0;
    let mut ok = true;
    while i < diff.len() {
        if matches!(diff[i], diff::Result::Both(_, _)) {
            i += 1;
            continue;
        }
        ok = false;
        let mut before = i;
        let mut before_lines = Vec::new();
        'outer: for _ in 0..2 {
            let line = loop {
                if before == 0 {
                    break 'outer;
                }
                before -= 1;
                let (diff::Result::Left(line) | diff::Result::Both(line, _)) = diff[before] else {
                    continue;
                };
                break line;
            };
            before_lines.push(line);
        }
        let mut content = String::new();
        while let Some(line) = diff.get(i) {
            match line {
                diff::Result::Left(l) => cwriteln!(content, "|  #r<-{}>", l).unwrap(),
                diff::Result::Right(r) => cwriteln!(content, "|  #g<+{}>", r).unwrap(),
                _ => break,
            }
            i += 1;
        }
        before_lines.reverse();
        let mut after = i + 1;
        let mut after_lines = Vec::new();
        'outer: for _ in 0..2 {
            let line = loop {
                if after >= diff.len() {
                    break 'outer;
                }
                let line = &diff[after];
                after += 1;
                let (diff::Result::Left(line) | diff::Result::Both(line, _)) = line else {
                    continue;
                };
                break line;
            };
            after_lines.push(*line);
        }
        ceprintln!("\n#bold<Diff in {}:{i}:>", file.display());
        for line in before_lines {
            eprintln!("| {line}");
        }
        eprint!("{content}");
        for line in after_lines {
            eprintln!("| {line}");
        }
    }
    ok
}
