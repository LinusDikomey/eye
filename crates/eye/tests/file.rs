use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
    sync::OnceLock,
};

use eye::args;
use test_each_file::test_each_path;

static TRACING_INIT: OnceLock<()> = OnceLock::new();

fn set_dir() {
    // this makes sure std is found and the normal top-level eyebuild directory is used
    std::env::set_current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../")).unwrap();
}

#[test]
fn check_std() {
    set_dir();
    let args = args::Args {
        cmd: args::Cmd::Check,
        path: Some("std".to_owned()),
        lib: true,
        ..Default::default()
    };
    eye::run(args).expect("std failed to check");
}

test_each_path! { for ["eye"] in "eye" => |[path]: [&Path; 1]| test_compile_and_run(path) }

fn test_compile_and_run(eye: &Path) {
    set_dir();
    let input = std::fs::read_to_string(eye.with_extension("in")).unwrap_or_default();
    let out = eye.with_extension("out");
    let err = eye.with_extension("err");
    let path = eye.to_str().unwrap().to_owned();
    let args = args::Args {
        cmd: args::Cmd::Build,
        path: Some(path.clone()),
        ..Default::default()
    };
    TRACING_INIT.get_or_init(|| eye::enable_tracing(&args));
    if out.exists() {
        eye::run(args).expect("Failed to compile");
        let output = run_executable(path, input);
        // TODO: not checking the exit status for now since some programs return other than 0
        // This should probably be defined in the .out file in these cases in the future
        // if !output.status.success() {
        //     panic!("Program returned with non-zero exit status");
        // }
        let output = String::from_utf8(output.stdout).unwrap();
        let expected = std::fs::read_to_string(out).unwrap();
        if output.trim() != expected.trim() {
            panic!("Output differed from the expected output:\n{output}");
        }
    } else if err.exists() {
        let result = eye::run(args);
        assert!(
            matches!(result, Err(eye::MainError::ErrorsFound)),
            "Errors during compilation expected but got {result:?}"
        );
        // TODO: .err should have a new format to list the specific errors and this should check
        // against them
    } else {
        panic!("No .out or .err file found");
    }
}

fn run_executable(path: String, input: String) -> std::process::Output {
    let (name, _) = eye::path_arg(Some(path)).unwrap();
    let exe_file = eye::exe_file_path(&name);
    let mut proc = Command::new(exe_file)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("Failed to execute built binary");
    if !input.is_empty() {
        let mut stdin = proc.stdin.take().unwrap();
        stdin.write_all(input.as_bytes()).unwrap();
        stdin.flush().unwrap();
    }
    proc.wait_with_output().expect("Process failed")
}

#[test]
/// Scans the README for code blocks to compils and run them, checking for successful exit
fn readme_code_blocks() {
    set_dir();
    let readme = std::fs::read_to_string("README.md").unwrap();
    // skip the initial text block and step into each code block
    for code in readme.split("\n```").skip(1).step_by(2) {
        let (_language, code) = code.split_once("\n").unwrap();
        println!("Testing README Code Block:\n{code}\n\n");
        std::fs::create_dir_all("eyebuild").unwrap();
        let tmp_path = "eyebuild/tmp.eye";
        std::fs::write(tmp_path, code).unwrap();
        eye::run(args::Args {
            cmd: args::Cmd::Build,
            path: Some(tmp_path.to_owned()),
            ..Default::default()
        })
        .expect("Failed to compile README block");
        let output = run_executable(tmp_path.to_owned(), String::new());
        if !output.status.success() {
            panic!("README test finished with a non-zero exit code")
        }
    }
}

#[test]
fn test_project() {
    set_dir();
    test_compile_and_run(Path::new("crates/eye/tests/test-project"));
}

#[test]
fn test_example() {
    set_dir();

    let path = "example.eye";
    let args = args::Args {
        cmd: args::Cmd::Build,
        path: Some(path.to_owned()),
        ..Default::default()
    };
    eye::run(args).expect("Failed to compile");
    let output = run_executable(path.to_owned(), String::new());
    assert!(output.status.success());
    let output = String::from_utf8(output.stdout).unwrap();
    let expected = "Hello, World\nArea of circle with radius 2.5: 19.62\nBanana";
    if output.trim() != expected.trim() {
        panic!("Output differed from the expected output:\n{output}");
    }
}
