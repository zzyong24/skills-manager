fn main() {
    // Ensure the CLI binary exists in `binaries/` before tauri-build checks for it.
    // tauri-build validates externalBin paths at compile time — if the file is missing
    // the build fails. We solve the chicken-and-egg problem by building the CLI binary
    // here in build.rs, which always runs before tauri_build::build().
    //
    // Anti-recursion: build.rs itself is invoked by cargo when building the main crate.
    // If we naively call `cargo build --bin skills-manager-cli`, that triggers build.rs
    // again in the subprocess, causing infinite nesting. We break the cycle by setting
    // SKILLS_MANAGER_BUILDING_CLI=1 in the subprocess environment — build.rs skips the
    // CLI build step when it detects this variable.
    build_cli_binary();

    tauri_build::build()
}

fn build_cli_binary() {
    use std::env;
    use std::path::PathBuf;
    use std::process::Command;

    // Break the recursion: if we're already inside a CLI build subprocess, skip.
    if env::var("SKILLS_MANAGER_BUILDING_CLI").is_ok() {
        return;
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    // Resolve target triple from cargo env
    let target_triple = env::var("TARGET").unwrap_or_else(|_| {
        Command::new("rustc")
            .args(["-vV"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("host:"))
                    .map(|l| l.trim_start_matches("host:").trim().to_string())
            })
            .unwrap_or_else(|| "unknown".to_string())
    });

    let ext = if target_triple.contains("windows") { ".exe" } else { "" };
    let bin_name = format!("skills-manager-cli-{}{}", target_triple, ext);
    let binaries_dir = manifest_dir.join("binaries");
    let dest = binaries_dir.join(&bin_name);

    // Already exists — nothing to do.
    if dest.exists() {
        println!("cargo:rerun-if-changed=binaries/{}", bin_name);
        return;
    }

    eprintln!("[build.rs] binaries/{} not found, building CLI binary...", bin_name);

    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
    let mut cmd = Command::new("cargo");
    cmd.current_dir(&manifest_dir)
        .env("SKILLS_MANAGER_BUILDING_CLI", "1")  // prevent recursion
        .args(["build", "--bin", "skills-manager-cli"]);
    if profile == "release" {
        cmd.arg("--release");
    }

    let status = cmd.status().expect("failed to run cargo build for skills-manager-cli");
    if !status.success() {
        panic!("Failed to build skills-manager-cli");
    }

    let built = manifest_dir
        .join("target")
        .join(&profile)
        .join(format!("skills-manager-cli{}", ext));

    std::fs::create_dir_all(&binaries_dir).expect("failed to create binaries/");
    std::fs::copy(&built, &dest).unwrap_or_else(|e| {
        panic!("Failed to copy {} -> {}: {}", built.display(), dest.display(), e)
    });

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dest).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&dest, perms).unwrap();
    }

    eprintln!("[build.rs] CLI binary ready at binaries/{}", bin_name);
    println!("cargo:rerun-if-changed=src/bin/skills-manager-cli.rs");
}
