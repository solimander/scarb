use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs, io};

use cargo_metadata::MetadataCommand;
use zip::ZipArchive;

fn main() {
    commit_info();
    let rev = cairo_version();
    download_core(&rev);
}

fn is_docs_rs() -> bool {
    env::var("DOCS_RS").is_ok()
}

fn commit_info() {
    if !Path::new("../.git").exists() {
        return;
    }
    println!("cargo:rerun-if-changed=../.git/index");
    let output = match Command::new("git")
        .arg("log")
        .arg("-1")
        .arg("--date=short")
        .arg("--format=%H %h %cd")
        .arg("--abbrev=9")
        .current_dir("..")
        .output()
    {
        Ok(output) if output.status.success() => output,
        _ => return,
    };
    let stdout = String::from_utf8(output.stdout).unwrap();
    let mut parts = stdout.split_whitespace();
    let mut next = || parts.next().unwrap();
    println!("cargo:rustc-env=SCARB_COMMIT_HASH={}", next());
    println!("cargo:rustc-env=SCARB_COMMIT_SHORT_HASH={}", next());
    println!("cargo:rustc-env=SCARB_COMMIT_DATE={}", next())
}

fn cairo_version() -> String {
    let cargo_lock = find_cargo_lock();
    println!("cargo:rerun-if-changed={}", cargo_lock.display());

    let metadata = MetadataCommand::new()
        .manifest_path(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .verbose(true)
        .exec()
        .expect("Failed to execute cargo metadata");

    let resolve = metadata
        .resolve
        .expect("Expected metadata resolve to be present.");

    let root = resolve
        .root
        .expect("Expected metadata resolve root to be present.");
    assert!(
        root.repr.starts_with("scarb "),
        "Expected metadata resolve root to be `scarb`."
    );

    let scarb_node = resolve.nodes.iter().find(|node| node.id == root).unwrap();
    let compiler_dep = scarb_node
        .deps
        .iter()
        .find(|dep| dep.name == "cairo_lang_compiler")
        .unwrap();
    let compiler_package = metadata
        .packages
        .iter()
        .find(|pkg| pkg.id == compiler_dep.pkg)
        .unwrap();

    let version = compiler_package.version.to_string();
    println!("cargo:rustc-env=SCARB_CAIRO_VERSION={version}");
    if let Some(source) = &compiler_package.source {
        let source = source.to_string();
        if source.starts_with("git+") {
            if let Some((_, commit)) = source.split_once('#') {
                println!("cargo:rustc-env=SCARB_CAIRO_COMMIT_HASH={commit}");
                return commit.to_string();
            }
        }
    }
    format!("refs/tags/v{version}")
}

fn download_core(rev: &str) {
    println!("cargo:rerun-if-env-changed=CAIRO_ARCHIVE");
    let out_dir = env::var("OUT_DIR").unwrap();
    if is_docs_rs() {
        eprintln!("Docs.rs build detected. Skipping corelib download.");
        let core_stub_path = PathBuf::from_iter([&out_dir, "core-stub"]);
        fs::create_dir_all(&core_stub_path).unwrap();
        println!(
            "cargo:rustc-env=SCARB_CORE_PATH={}",
            core_stub_path.display()
        );
        return;
    }

    let core_path = PathBuf::from_iter([&out_dir, &format!("core-{}", ident(rev))]);
    if !core_path.is_dir() {
        let cairo_zip = PathBuf::from_iter([&out_dir, "cairo.zip"]);

        if let Ok(cairo_archive) = std::env::var("CAIRO_ARCHIVE") {
            // Copy archive to `cairo_zip`, without keeping file attributes.
            eprintln!("Copying Cairo archive from `CAIRO_ARCHIVE={cairo_archive}`.");
            let mut src = File::open(&cairo_archive).unwrap();
            let mut dst = File::create(&cairo_zip).unwrap();
            io::copy(&mut src, &mut dst).unwrap();
        } else {
            let url = format!("https://github.com/starkware-libs/cairo/archive/{rev}.zip");
            let mut curl = Command::new("curl");
            curl.args(["--proto", "=https", "--tlsv1.2", "-fL"]);
            curl.arg("-o");
            curl.arg(&cairo_zip);
            curl.arg(&url);
            eprintln!("{curl:?}");
            let curl_exit = curl.status().expect("Failed to start curl");
            if !curl_exit.success() {
                panic!("Failed to download {url} with curl")
            }
        }

        fs::create_dir_all(&core_path).unwrap();
        let cairo_file = File::open(cairo_zip).unwrap();
        let mut cairo_archive = ZipArchive::new(cairo_file).unwrap();
        for i in 0..cairo_archive.len() {
            let mut input = cairo_archive.by_index(i).unwrap();

            if input.name().ends_with('/') {
                continue;
            }

            let path = input.enclosed_name().unwrap();

            let path = PathBuf::from_iter(path.components().skip(1));
            let Ok(path) = path.strip_prefix("corelib") else {
                continue;
            };

            let path = core_path.join(path);

            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }

            let mut output = File::create(path).unwrap();
            io::copy(&mut input, &mut output).unwrap();
        }
    }

    println!("cargo:rustc-env=SCARB_CORE_PATH={}", core_path.display());
}

fn ident(id: &str) -> String {
    let mut ident = String::with_capacity(id.len());
    for ch in id.chars() {
        ident.push(if ch.is_ascii_alphanumeric() { ch } else { '_' })
    }
    ident
}

fn find_cargo_lock() -> PathBuf {
    let in_workspace = PathBuf::from("../Cargo.lock");
    if in_workspace.exists() {
        return in_workspace;
    }

    let in_package = PathBuf::from("Cargo.lock");
    if in_package.exists() {
        return in_package;
    }

    panic!(
        "Couldn't find Cargo.lock of this package. \
        Something's wrong with build execution environment."
    )
}
