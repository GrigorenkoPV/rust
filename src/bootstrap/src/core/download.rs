use std::collections::HashMap;
use std::env;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use build_helper::ci::CiEnv;
use build_helper::git::PathFreshness;
use build_helper::stage0_parser::VersionMetadata;
use xz2::bufread::XzDecoder;

use crate::core::build_steps::llvm::detect_llvm_freshness;
use crate::core::config::toml::llvm::check_incompatible_options_for_ci_llvm;
use crate::core::config::{BUILDER_CONFIG_FILENAME, Config, TargetSelection};
use crate::utils::build_stamp::BuildStamp;
use crate::utils::exec::{ExecutionContext, command};
use crate::utils::helpers::{self, exe, hex_encode, move_file, t};

static SHOULD_FIX_BINS_AND_DYLIBS: OnceLock<bool> = OnceLock::new();

fn extract_curl_version(out: String) -> semver::Version {
    // The output should look like this: "curl <major>.<minor>.<patch> ..."
    out.lines()
        .next()
        .and_then(|line| line.split(" ").nth(1))
        .and_then(|version| semver::Version::parse(version).ok())
        .unwrap_or(semver::Version::new(1, 0, 0))
}

/// Generic helpers that are useful anywhere in bootstrap.
impl Config {
    pub fn is_verbose(&self) -> bool {
        self.exec_ctx.is_verbose()
    }

    pub(crate) fn create<P: AsRef<Path>>(&self, path: P, s: &str) {
        if self.dry_run() {
            return;
        }
        t!(fs::write(path, s));
    }

    pub(crate) fn remove(&self, f: &Path) {
        remove(&self.exec_ctx, f);
    }

    /// Create a temporary directory in `out` and return its path.
    ///
    /// NOTE: this temporary directory is shared between all steps;
    /// if you need an empty directory, create a new subdirectory inside it.
    pub(crate) fn tempdir(&self) -> PathBuf {
        let tmp = self.out.join("tmp");
        t!(fs::create_dir_all(&tmp));
        tmp
    }

    fn download_file(&self, url: &str, dest_path: &Path, help_on_error: &str) {
        let dwn_ctx: DownloadContext<'_> = self.into();
        download_file(dwn_ctx, &self.out, url, dest_path, help_on_error);
    }

    fn unpack(&self, tarball: &Path, dst: &Path, pattern: &str) {
        unpack(&self.exec_ctx, tarball, dst, pattern);
    }

    /// Returns whether the SHA256 checksum of `path` matches `expected`.
    #[cfg(test)]
    pub(crate) fn verify(&self, path: &Path, expected: &str) -> bool {
        verify(&self.exec_ctx, path, expected)
    }
}

fn recorded_entries(dst: &Path, pattern: &str) -> Option<BufWriter<File>> {
    let name = if pattern == "rustc-dev" {
        ".rustc-dev-contents"
    } else if pattern.starts_with("rust-std") {
        ".rust-std-contents"
    } else {
        return None;
    };
    Some(BufWriter::new(t!(File::create(dst.join(name)))))
}

#[derive(Clone)]
enum DownloadSource {
    CI,
    Dist,
}

/// Functions that are only ever called once, but named for clarity and to avoid thousand-line functions.
impl Config {
    pub(crate) fn download_clippy(&self, initial_sysroot: &Path) -> PathBuf {
        self.do_if_verbose(|| println!("downloading stage0 clippy artifacts"));

        let date = &self.stage0_metadata.compiler.date;
        let version = &self.stage0_metadata.compiler.version;
        let host = self.host_target;
        let clippy_stamp = BuildStamp::new(initial_sysroot).with_prefix("clippy").add_stamp(date);
        let cargo_clippy_name = exe("cargo-clippy", host);
        let bin_dir = initial_sysroot.join("bin");
        let cargo_clippy = bin_dir.join(&cargo_clippy_name);

        if cargo_clippy.exists() && clippy_stamp.is_up_to_date() {
            return cargo_clippy;
        }

        let filename = format!("clippy-{version}-{host}.tar.xz");
        self.download_component(DownloadSource::Dist, filename, "clippy-preview", date, "stage0");

        if self.should_patchelf() {
            let clippy_driver_name = exe("clippy-driver", host);
            self.patchelfs(&bin_dir, &[&cargo_clippy_name, &clippy_driver_name]);
        }

        t!(clippy_stamp.write());
        cargo_clippy
    }

    pub(crate) fn ci_rust_std_contents(&self) -> Vec<String> {
        self.ci_component_contents(".rust-std-contents")
    }

    pub(crate) fn ci_rustc_dev_contents(&self) -> Vec<String> {
        self.ci_component_contents(".rustc-dev-contents")
    }

    fn ci_component_contents(&self, stamp_file: &str) -> Vec<String> {
        assert!(self.download_rustc());
        if self.dry_run() {
            return vec![];
        }

        let ci_rustc_dir = self.ci_rustc_dir();
        let stamp_file = ci_rustc_dir.join(stamp_file);
        let contents_file = t!(File::open(&stamp_file), stamp_file.display().to_string());
        t!(BufReader::new(contents_file).lines().collect())
    }

    pub(crate) fn download_ci_rustc(&self, commit: &str) {
        self.do_if_verbose(|| {
            println!("using downloaded stage2 artifacts from CI (commit {commit})")
        });

        let version = self.artifact_version_part(commit);
        // download-rustc doesn't need its own cargo, it can just use beta's. But it does need the
        // `rustc_private` crates for tools.
        let extra_components = ["rustc-dev"];

        self.download_toolchain(
            &version,
            "ci-rustc",
            &format!("{commit}-{}", self.llvm_assertions),
            &extra_components,
            Self::download_ci_component,
        );
    }

    pub(crate) fn download_std_json_docs(
        &self,
        target: TargetSelection,
        commit: &str,
    ) -> Option<PathBuf> {
        if self.dry_run() {
            return None;
        }

        self.do_if_verbose(|| println!("using downloaded std json docs from CI (commit {commit})"));

        let version = self.artifact_version_part(commit);
        download_component(
            DownloadContext::from(self),
            &self.out,
            DownloadSource::CI,
            format!("rust-docs-json-{version}-{target}.tar.xz"),
            "rust-docs-json-preview",
            // When using DownloadSource::CI, the key is assumed to end with -llvm-assertions
            &format!("{commit}-{}", self.llvm_assertions),
            "ci-docs-json",
        )
    }

    fn download_toolchain(
        &self,
        version: &str,
        sysroot: &str,
        stamp_key: &str,
        extra_components: &[&str],
        download_component: fn(&Config, String, &str, &str),
    ) {
        let host = self.host_target.triple;
        let bin_root = self.out.join(host).join(sysroot);
        let rustc_stamp = BuildStamp::new(&bin_root).with_prefix("rustc").add_stamp(stamp_key);

        if !bin_root.join("bin").join(exe("rustc", self.host_target)).exists()
            || !rustc_stamp.is_up_to_date()
        {
            if bin_root.exists() {
                t!(fs::remove_dir_all(&bin_root));
            }
            let filename = format!("rust-std-{version}-{host}.tar.xz");
            let pattern = format!("rust-std-{host}");
            download_component(self, filename, &pattern, stamp_key);
            let filename = format!("rustc-{version}-{host}.tar.xz");
            download_component(self, filename, "rustc", stamp_key);

            for component in extra_components {
                let filename = format!("{component}-{version}-{host}.tar.xz");
                download_component(self, filename, component, stamp_key);
            }

            if self.should_patchelf() {
                self.patchelfs(&bin_root.join("bin"), &["rustc", "rustdoc"]);
                self.patchelfs(&bin_root.join("libexec"), &["rust-analyzer-proc-macro-srv"]);
                self.patchelf_all_so(&bin_root.join("lib"));
            }

            t!(rustc_stamp.write());
        }
    }

    /// Download a single component of a CI-built toolchain (not necessarily a published nightly).
    // NOTE: intentionally takes an owned string to avoid downloading multiple times by accident
    fn download_ci_component(&self, filename: String, prefix: &str, commit_with_assertions: &str) {
        Self::download_component(
            self,
            DownloadSource::CI,
            filename,
            prefix,
            commit_with_assertions,
            "ci-rustc",
        )
    }

    fn download_component(
        &self,
        mode: DownloadSource,
        filename: String,
        prefix: &str,
        key: &str,
        destination: &str,
    ) {
        let dwn_ctx: DownloadContext<'_> = self.into();
        download_component(dwn_ctx, &self.out, mode, filename, prefix, key, destination);
    }

    /// Attempts to download LLVM from CI for the given **target**.
    /// Returns a path to the downloaded and extracted directory.
    pub(crate) fn maybe_download_ci_llvm(&self, target: TargetSelection) -> Option<PathBuf> {
        // Never try to download CI LLVM during unit tests.
        if cfg!(test) {
            return None;
        }

        let llvm_root = self.out.join(target).join("ci-llvm");
        let llvm_freshness =
            detect_llvm_freshness(self, self.rust_info.is_managed_git_subrepository());
        self.do_if_verbose(|| {
            eprintln!("LLVM freshness: {llvm_freshness:?}");
        });
        let llvm_sha = match llvm_freshness {
            PathFreshness::LastModifiedUpstream { upstream } => upstream,
            PathFreshness::HasLocalModifications { upstream, modifications: _ } => upstream,
            PathFreshness::MissingUpstream => {
                eprintln!("error: could not find commit hash for downloading LLVM");
                eprintln!("HELP: maybe your repository history is too shallow?");
                eprintln!("HELP: consider disabling `download-ci-llvm`");
                eprintln!("HELP: or fetch enough history to include one upstream commit");
                helpers::exit_process(1);
            }
        };
        let stamp_key = format!("{}{}", llvm_sha, self.llvm_assertions);
        let llvm_stamp = BuildStamp::new(&llvm_root).with_prefix("llvm").add_stamp(stamp_key);
        if !llvm_stamp.is_up_to_date() && !self.dry_run() {
            self.download_ci_llvm(&llvm_root, target, &llvm_sha);

            self.patchelf_all(&llvm_root.join("bin"));

            // Update the timestamp of llvm-config to force rustc_llvm to be
            // rebuilt. This is a hacky workaround for a deficiency in Cargo where
            // the rerun-if-changed directive doesn't handle changes very well.
            // https://github.com/rust-lang/cargo/issues/10791
            // Cargo only compares the timestamp of the file relative to the last
            // time `rustc_llvm` build script ran. However, the timestamps of the
            // files in the tarball are in the past, so it doesn't trigger a
            // rebuild.
            let now = std::time::SystemTime::now();
            let file_times = fs::FileTimes::new().set_accessed(now).set_modified(now);

            let llvm_config = llvm_root.join("bin").join(exe("llvm-config", target));
            t!(crate::utils::helpers::set_file_times(llvm_config, file_times));

            self.patchelf_all_so(&llvm_root.join("lib"));

            t!(llvm_stamp.write());
        }

        if let Some(config_path) = &self.config {
            let current_config_toml = Self::get_toml(config_path).unwrap();

            match self.get_builder_toml("ci-llvm") {
                Ok(ci_config_toml) => {
                    t!(check_incompatible_options_for_ci_llvm(current_config_toml, ci_config_toml));
                }
                Err(e) if e.to_string().contains("unknown field") => {
                    println!(
                        "WARNING: CI LLVM has some fields that are no longer supported in bootstrap; download-ci-llvm will be disabled."
                    );
                    println!("HELP: Consider rebasing to a newer commit if available.");
                }
                Err(e) => {
                    eprintln!("ERROR: Failed to parse CI LLVM bootstrap.toml: {e}");
                    helpers::exit_process(2);
                }
            };
        };
        Some(llvm_root)
    }

    fn download_ci_llvm(&self, llvm_root: &Path, target: TargetSelection, llvm_sha: &str) {
        // For unit tests, downloading should have been blocked by `maybe_download_ci_llvm`.
        assert!(cfg!(not(test)), "unit tests shouldn't be downloading CI LLVM");

        let llvm_assertions = self.llvm_assertions;

        let cache_prefix = format!("llvm-{}-{llvm_sha}-{llvm_assertions}", target.triple);
        let cache_dst =
            self.bootstrap_cache_path.as_ref().cloned().unwrap_or_else(|| self.out.join("cache"));

        let rustc_cache = cache_dst.join(cache_prefix);
        if !rustc_cache.exists() {
            t!(fs::create_dir_all(&rustc_cache));
        }
        let base = if llvm_assertions {
            &self.stage0_metadata.config.artifacts_with_llvm_assertions_server
        } else {
            &self.stage0_metadata.config.artifacts_server
        };
        let version = self.artifact_version_part(llvm_sha);
        let filename = format!("rust-dev-{}-{}.tar.xz", version, target.triple);
        let tarball = rustc_cache.join(&filename);
        if !tarball.exists() {
            let help_on_error = format!(
                "ERROR: failed to download llvm from CI for `{target}`

    HELP: There could be two reasons behind this:
        1) `{target}` is not supported for `download-ci-llvm`.
        2) Old builds get deleted after a certain time.
    HELP: In either case, disable `download-ci-llvm` in your bootstrap.toml:

    [llvm]
    download-ci-llvm = false
    ",
                target = target.triple
            );
            self.download_file(&format!("{base}/{llvm_sha}/{filename}"), &tarball, &help_on_error);
        }
        self.unpack(&tarball, llvm_root, "rust-dev");
    }

    pub fn download_ci_gcc(&self, gcc_sha: &str, root_dir: &Path) {
        let cache_prefix = format!("gcc-{gcc_sha}");
        let cache_dst =
            self.bootstrap_cache_path.as_ref().cloned().unwrap_or_else(|| self.out.join("cache"));

        let gcc_cache = cache_dst.join(cache_prefix);
        if !gcc_cache.exists() {
            t!(fs::create_dir_all(&gcc_cache));
        }
        let base = &self.stage0_metadata.config.artifacts_server;
        let version = self.artifact_version_part(gcc_sha);
        let filename = format!("gcc-dev-{version}-{}.tar.xz", self.host_target.triple);
        let tarball = gcc_cache.join(&filename);
        if !tarball.exists() {
            let help_on_error = "ERROR: failed to download gcc from ci

    HELP: There could be two reasons behind this:
        1) The host triple is not supported for `download-ci-gcc`.
        2) Old builds get deleted after a certain time.
    HELP: In either case, disable `download-ci-gcc` in your bootstrap.toml:

    [gcc]
    download-ci-gcc = false
    ";
            self.download_file(&format!("{base}/{gcc_sha}/{filename}"), &tarball, help_on_error);
        }
        self.unpack(&tarball, root_dir, "gcc-dev");

        self.patchelf_all_so(&root_dir.join("lib"));
    }
}

/// Only should be used for pre config initialization downloads.
pub(crate) struct DownloadContext<'a> {
    pub path_modification_cache: Arc<Mutex<HashMap<Vec<&'static str>, PathFreshness>>>,
    pub src: &'a Path,
    pub submodules: &'a Option<bool>,
    pub host_target: TargetSelection,
    pub patch_binaries_for_nix: Option<bool>,
    pub exec_ctx: &'a ExecutionContext,
    pub stage0_metadata: &'a build_helper::stage0_parser::Stage0,
    pub llvm_assertions: bool,
    pub bootstrap_cache_path: &'a Option<PathBuf>,
    pub ci_env: CiEnv,
}

impl<'a> DownloadContext<'a> {
    pub fn is_running_on_ci(&self) -> bool {
        self.ci_env.is_running_in_ci()
    }
}

impl<'a> AsRef<DownloadContext<'a>> for DownloadContext<'a> {
    fn as_ref(&self) -> &DownloadContext<'a> {
        self
    }
}

impl<'a> From<&'a Config> for DownloadContext<'a> {
    fn from(value: &'a Config) -> Self {
        DownloadContext {
            path_modification_cache: value.path_modification_cache.clone(),
            src: &value.src,
            host_target: value.host_target,
            submodules: &value.submodules,
            patch_binaries_for_nix: value.patch_binaries_for_nix,
            exec_ctx: &value.exec_ctx,
            stage0_metadata: &value.stage0_metadata,
            llvm_assertions: value.llvm_assertions,
            bootstrap_cache_path: &value.bootstrap_cache_path,
            ci_env: value.ci_env,
        }
    }
}

fn path_is_dylib(path: &Path) -> bool {
    // The .so is not necessarily the extension, it might be libLLVM.so.18.1
    path.to_str().is_some_and(|path| path.contains(".so"))
}

/// Checks whether the CI rustc is available for the given target triple.
pub(crate) fn is_download_ci_available(target_triple: &str, llvm_assertions: bool) -> bool {
    // All tier 1 targets and tier 2 targets with host tools.
    const SUPPORTED_PLATFORMS: &[&str] = &[
        "aarch64-apple-darwin",
        "aarch64-pc-windows-gnullvm",
        "aarch64-pc-windows-msvc",
        "aarch64-unknown-linux-gnu",
        "aarch64-unknown-linux-musl",
        "arm-unknown-linux-gnueabi",
        "arm-unknown-linux-gnueabihf",
        "armv7-unknown-linux-gnueabihf",
        "i686-pc-windows-gnu",
        "i686-pc-windows-msvc",
        "i686-unknown-linux-gnu",
        "loongarch64-unknown-linux-gnu",
        "powerpc-unknown-linux-gnu",
        "powerpc64-unknown-linux-gnu",
        "powerpc64-unknown-linux-musl",
        "powerpc64le-unknown-linux-gnu",
        "powerpc64le-unknown-linux-musl",
        "riscv64gc-unknown-linux-gnu",
        "riscv64gc-unknown-linux-musl",
        "s390x-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "x86_64-pc-windows-gnu",
        "x86_64-pc-windows-gnullvm",
        "x86_64-pc-windows-msvc",
        "x86_64-unknown-freebsd",
        "x86_64-unknown-illumos",
        "x86_64-unknown-linux-gnu",
        "x86_64-unknown-linux-musl",
        "x86_64-unknown-netbsd",
    ];

    const SUPPORTED_PLATFORMS_WITH_ASSERTIONS: &[&str] =
        &["x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc"];

    if llvm_assertions {
        SUPPORTED_PLATFORMS_WITH_ASSERTIONS.contains(&target_triple)
    } else {
        SUPPORTED_PLATFORMS.contains(&target_triple)
    }
}

/// NOTE: rustfmt is a completely different toolchain than the bootstrap compiler, so it can't
/// reuse target directories or artifacts
pub(crate) fn maybe_download_rustfmt(config: &Config, out: &Path) -> Option<PathBuf> {
    // Don't actually download rustfmt during unit tests.
    if cfg!(test) {
        return Some(PathBuf::new());
    }

    if config.dry_run() {
        return Some(PathBuf::new());
    }

    let VersionMetadata { date, version, .. } = config.stage0_metadata.rustfmt.as_ref()?;
    let channel = format!("{version}-{date}");

    let host = config.host_target;
    let bin_root = out.join(host).join("rustfmt");
    let rustfmt_path = bin_root.join("bin").join(exe("rustfmt", host));
    let rustfmt_stamp = BuildStamp::new(&bin_root).with_prefix("rustfmt").add_stamp(channel);
    if rustfmt_path.exists() && rustfmt_stamp.is_up_to_date() {
        return Some(rustfmt_path);
    }

    download_component(
        DownloadContext::from(config),
        out,
        DownloadSource::Dist,
        format!("rustfmt-{version}-{build}.tar.xz", build = host.triple),
        "rustfmt-preview",
        date,
        "rustfmt",
    );

    download_component(
        DownloadContext::from(config),
        out,
        DownloadSource::Dist,
        format!("rustc-{version}-{build}.tar.xz", build = host.triple),
        "rustc",
        date,
        "rustfmt",
    );

    if config.should_patchelf() {
        config.patchelfs(&bin_root.join("bin"), &["rustfmt", "cargo-fmt"]);
        config.patchelf_all_so(&bin_root.join("lib"));
    }

    t!(rustfmt_stamp.write());
    Some(rustfmt_path)
}

pub(crate) fn download_beta_toolchain<'a>(dwn_ctx: impl AsRef<DownloadContext<'a>>, out: &Path) {
    // Don't actually download a beta toolchain during unit tests.
    if cfg!(test) {
        return;
    }

    let dwn_ctx = dwn_ctx.as_ref();
    dwn_ctx.exec_ctx.do_if_verbose(|| {
        println!("downloading stage0 beta artifacts");
    });

    let date = dwn_ctx.stage0_metadata.compiler.date.clone();
    let version = dwn_ctx.stage0_metadata.compiler.version.clone();
    let extra_components = ["cargo"];
    let sysroot = "stage0";
    download_toolchain(
        dwn_ctx,
        out,
        &version,
        sysroot,
        &date,
        &extra_components,
        "stage0",
        DownloadSource::Dist,
    );
}

#[allow(clippy::too_many_arguments)]
fn download_toolchain<'a>(
    dwn_ctx: impl AsRef<DownloadContext<'a>>,
    out: &Path,
    version: &str,
    sysroot: &str,
    stamp_key: &str,
    extra_components: &[&str],
    destination: &str,
    mode: DownloadSource,
) {
    assert!(cfg!(not(test)), "unit tests shouldn't be downloading a toolchain");

    let dwn_ctx = dwn_ctx.as_ref();
    let host = dwn_ctx.host_target.triple;
    let bin_root = out.join(host).join(sysroot);
    let rustc_stamp = BuildStamp::new(&bin_root).with_prefix("rustc").add_stamp(stamp_key);

    if !bin_root.join("bin").join(exe("rustc", dwn_ctx.host_target)).exists()
        || !rustc_stamp.is_up_to_date()
    {
        if bin_root.exists() {
            t!(fs::remove_dir_all(&bin_root));
        }
        let filename = format!("rust-std-{version}-{host}.tar.xz");
        let pattern = format!("rust-std-{host}");
        download_component(dwn_ctx, out, mode.clone(), filename, &pattern, stamp_key, destination);
        let filename = format!("rustc-{version}-{host}.tar.xz");
        download_component(dwn_ctx, out, mode.clone(), filename, "rustc", stamp_key, destination);

        for component in extra_components {
            let filename = format!("{component}-{version}-{host}.tar.xz");
            download_component(
                dwn_ctx,
                out,
                mode.clone(),
                filename,
                component,
                stamp_key,
                destination,
            );
        }

        let ctx = (dwn_ctx, out);
        if ctx.should_patchelf() {
            ctx.patchelfs(&bin_root.join("bin"), &["rustc", "rustdoc"]);
            ctx.patchelfs(&bin_root.join("libexec"), &["rust-analyzer-proc-macro-srv"]);
            ctx.patchelf_all_so(&bin_root.join("lib"));
        }

        t!(rustc_stamp.write());
    }
}

pub(crate) fn remove(exec_ctx: &ExecutionContext, f: &Path) {
    if exec_ctx.dry_run() {
        return;
    }
    fs::remove_file(f).unwrap_or_else(|_| panic!("failed to remove {f:?}"));
}

// `fix_bin_or_dylib` from bootstrap.py
trait PatchelfContext {
    fn should_patch_binaries_for_nix(&self) -> Option<bool>;
    fn get_exec_ctx(&self) -> &ExecutionContext;
    fn get_out_dir(&self) -> &Path;

    #[must_use]
    // FIXME(https://github.com/rust-lang/rust/issues/131179): #[final]
    fn should_patchelf(&self) -> bool {
        *SHOULD_FIX_BINS_AND_DYLIBS.get_or_init(|| {
            let uname =
                command("uname").allow_failure().arg("-s").run_capture_stdout(self.get_exec_ctx());
            if uname.is_failure() {
                return false;
            }
            let output = uname.stdout();
            if !output.starts_with("Linux") {
                return false;
            }
            // If the user has asked binaries to be patched for Nix, then
            // don't check for NixOS or `/lib`.
            // NOTE: this intentionally comes after the Linux check:
            // - patchelf only works with ELF files, so no need to run it on Mac or Windows
            // - On other Unix systems, there is no stable syscall interface, so Nix doesn't manage the global libc.
            if let Some(explicit_value) = self.should_patch_binaries_for_nix() {
                return explicit_value;
            }

            // Use `/etc/os-release` instead of `/etc/NIXOS`.
            // The latter one does not exist on NixOS when using tmpfs as root.
            let is_nixos = match File::open("/etc/os-release") {
                Err(e) if e.kind() == ErrorKind::NotFound => false,
                Err(e) => panic!("failed to access /etc/os-release: {e}"),
                Ok(os_release) => BufReader::new(os_release).lines().any(|l| {
                    let l = l.expect("reading /etc/os-release");
                    matches!(l.trim(), "ID=nixos" | "ID='nixos'" | "ID=\"nixos\"")
                }),
            };
            if is_nixos {
                eprintln!("INFO: You seem to be using Nix.");
            } else {
                if let Ok(in_nix_shell) = env::var("IN_NIX_SHELL") {
                    eprintln!(
                        "The IN_NIX_SHELL environment variable is `{in_nix_shell}`; \
                     you may need to set `patch-binaries-for-nix=true` in bootstrap.toml"
                    );
                }
            }
            is_nixos
        })
    }

    // FIXME(https://github.com/rust-lang/rust/issues/131179): #[final]
    fn patchelfs(&self, dir: &Path, entries: &[&str]) {
        if !self.should_patchelf() {
            return;
        }
        for entry in entries {
            fix_bin_or_dylib(self.get_out_dir(), &dir.join(entry), self.get_exec_ctx());
        }
    }

    // FIXME(https://github.com/rust-lang/rust/issues/131179): #[final]
    /// Patches files in `dir` (only one level deep) that look like dylibs.
    fn patchelf_all_so(&self, dir: &Path) {
        self.patchelf_some(dir, path_is_dylib)
    }

    // FIXME(https://github.com/rust-lang/rust/issues/131179): #[final]
    /// Patches files in `dir` (only one level deep).
    fn patchelf_all(&self, dir: &Path) {
        self.patchelf_some(dir, |_| true)
    }

    // FIXME(https://github.com/rust-lang/rust/issues/131179): #[final]
    /// Patches files in `dir` (only one level deep) if they satisfy a given predicate.
    fn patchelf_some(&self, dir: &Path, filter: impl Fn(&Path) -> bool) {
        if !self.should_patchelf() {
            return;
        }
        for entry in t!(fs::read_dir(dir), dir.display().to_string()) {
            let path = t!(entry).path();
            if filter(&path) {
                fix_bin_or_dylib(self.get_out_dir(), &path, self.get_exec_ctx());
            }
        }
    }
}

impl PatchelfContext for (&DownloadContext<'_>, &Path) {
    fn should_patch_binaries_for_nix(&self) -> Option<bool> {
        self.0.patch_binaries_for_nix
    }

    fn get_exec_ctx(&self) -> &ExecutionContext {
        self.0.exec_ctx
    }

    fn get_out_dir(&self) -> &Path {
        self.1
    }
}

impl PatchelfContext for Config {
    fn should_patch_binaries_for_nix(&self) -> Option<bool> {
        self.patch_binaries_for_nix
    }

    fn get_exec_ctx(&self) -> &ExecutionContext {
        &self.exec_ctx
    }

    fn get_out_dir(&self) -> &Path {
        &self.out
    }
}

/// Modifies the interpreter section of 'fname' to fix the dynamic linker,
/// or the RPATH section, to fix the dynamic library search path
///
/// This is only required on NixOS and uses the PatchELF utility to
/// change the interpreter/RPATH of ELF executables.
///
/// Please see <https://nixos.org/patchelf.html> for more information
fn fix_bin_or_dylib(out: &Path, fname: &Path, exec_ctx: &ExecutionContext) {
    assert_eq!(SHOULD_FIX_BINS_AND_DYLIBS.get(), Some(&true));
    println!("attempting to patch {}", fname.display());

    // Only build `.nix-deps` once.
    static NIX_DEPS_DIR: OnceLock<PathBuf> = OnceLock::new();
    let mut nix_build_succeeded = true;
    let nix_deps_dir = NIX_DEPS_DIR.get_or_init(|| {
        // Run `nix-build` to "build" each dependency (which will likely reuse
        // the existing `/nix/store` copy, or at most download a pre-built copy).
        //
        // Importantly, we create a gc-root called `.nix-deps` in the `build/`
        // directory, but still reference the actual `/nix/store` path in the rpath
        // as it makes it significantly more robust against changes to the location of
        // the `.nix-deps` location.
        //
        // bintools: Needed for the path of `ld-linux.so` (via `nix-support/dynamic-linker`).
        // cc.lib: Needed similarly for `libstdc++.so.6`.
        // zlib: Needed as a system dependency of `libLLVM-*.so`.
        // zstd.out: Needed as a system dependency of `libgccjit.so`. `.out` is necessary as the
        //           default output of `zstd` derivation is `.bin`.
        // patchelf: Needed for patching ELF binaries (see doc comment above).
        let nix_deps_dir = out.join(".nix-deps");
        const NIX_EXPR: &str = "
        with (import <nixpkgs> {});
        symlinkJoin {
            name = \"rust-stage0-dependencies\";
            paths = [
                zlib
                zstd.out
                patchelf
                stdenv.cc.bintools
                stdenv.cc.cc.lib
            ];
        }
        ";
        nix_build_succeeded = command("nix-build")
            .allow_failure()
            .args([Path::new("-E"), Path::new(NIX_EXPR), Path::new("-o"), &nix_deps_dir])
            .run_capture_stdout(exec_ctx)
            .is_success();
        nix_deps_dir
    });
    if !nix_build_succeeded {
        return;
    }

    let mut patchelf = command(nix_deps_dir.join("bin/patchelf"));
    patchelf.args(&[
        OsString::from("--add-rpath"),
        OsString::from(t!(fs::canonicalize(nix_deps_dir)).join("lib")),
    ]);
    if !path_is_dylib(fname) {
        // Finally, set the correct .interp for binaries
        let dynamic_linker_path = nix_deps_dir.join("nix-support/dynamic-linker");
        let dynamic_linker = t!(fs::read_to_string(dynamic_linker_path));
        patchelf.args(["--set-interpreter", dynamic_linker.trim_end()]);
    }
    patchelf.arg(fname);
    let _ = patchelf.allow_failure().run_capture_stdout(exec_ctx);
}

fn download_component<'a>(
    dwn_ctx: impl AsRef<DownloadContext<'a>>,
    out: &Path,
    mode: DownloadSource,
    filename: String,
    prefix: &str,
    key: &str,
    destination: &str,
) -> Option<PathBuf> {
    let dwn_ctx = dwn_ctx.as_ref();

    if dwn_ctx.exec_ctx.dry_run() {
        return None;
    }

    let cache_dst =
        dwn_ctx.bootstrap_cache_path.as_ref().cloned().unwrap_or_else(|| out.join("cache"));

    let cache_dir = cache_dst.join(key);
    if !cache_dir.exists() {
        t!(fs::create_dir_all(&cache_dir));
    }

    let bin_root = out.join(dwn_ctx.host_target).join(destination);
    let tarball = cache_dir.join(&filename);
    let (base_url, url, should_verify) = match mode {
        DownloadSource::CI => {
            let dist_server = if dwn_ctx.llvm_assertions {
                dwn_ctx.stage0_metadata.config.artifacts_with_llvm_assertions_server.clone()
            } else {
                dwn_ctx.stage0_metadata.config.artifacts_server.clone()
            };
            let url = format!(
                "{}/{filename}",
                key.strip_suffix(&format!("-{}", dwn_ctx.llvm_assertions)).unwrap()
            );
            (dist_server, url, false)
        }
        DownloadSource::Dist => {
            let dist_server = env::var("RUSTUP_DIST_SERVER")
                .unwrap_or(dwn_ctx.stage0_metadata.config.dist_server.to_string());
            // NOTE: make `dist` part of the URL because that's how it's stored in src/stage0
            (dist_server, format!("dist/{key}/{filename}"), true)
        }
    };

    // For the stage0 compiler, put special effort into ensuring the checksums are valid.
    let checksum = if should_verify {
        let error = format!(
            "src/stage0 doesn't contain a checksum for {url}. \
            Pre-built artifacts might not be available for this \
            target at this time, see https://doc.rust-lang.org/nightly\
            /rustc/platform-support.html for more information."
        );
        let sha256 = dwn_ctx.stage0_metadata.checksums_sha256.get(&url).expect(&error);
        if tarball.exists() {
            if verify(dwn_ctx.exec_ctx, &tarball, sha256) {
                return Some(unpack(dwn_ctx.exec_ctx, &tarball, &bin_root, prefix));
            } else {
                dwn_ctx.exec_ctx.do_if_verbose(|| {
                    println!(
                        "ignoring cached file {} due to failed verification",
                        tarball.display()
                    )
                });
                remove(dwn_ctx.exec_ctx, &tarball);
            }
        }
        Some(sha256)
    } else if tarball.exists() {
        return Some(unpack(dwn_ctx.exec_ctx, &tarball, &bin_root, prefix));
    } else {
        None
    };

    let mut help_on_error = "";
    if destination == "ci-rustc" {
        help_on_error = "ERROR: failed to download pre-built rustc from CI

NOTE: old builds get deleted after a certain time
HELP: if trying to compile an old commit of rustc, disable `download-rustc` in bootstrap.toml:

[rust]
download-rustc = false
";
    }
    download_file(dwn_ctx, out, &format!("{base_url}/{url}"), &tarball, help_on_error);
    if let Some(sha256) = checksum
        && !verify(dwn_ctx.exec_ctx, &tarball, sha256)
    {
        panic!("failed to verify {}", tarball.display());
    }

    Some(unpack(dwn_ctx.exec_ctx, &tarball, &bin_root, prefix))
}

pub(crate) fn verify(exec_ctx: &ExecutionContext, path: &Path, expected: &str) -> bool {
    use sha2::Digest;

    exec_ctx.do_if_verbose(|| {
        println!("verifying {}", path.display());
    });

    if exec_ctx.dry_run() {
        return false;
    }

    let mut hasher = sha2::Sha256::new();

    let file = t!(File::open(path));
    let mut reader = BufReader::new(file);

    loop {
        let buffer = t!(reader.fill_buf());
        let l = buffer.len();
        // break if EOF
        if l == 0 {
            break;
        }
        hasher.update(buffer);
        reader.consume(l);
    }

    let checksum = hex_encode(hasher.finalize().as_slice());
    let verified = checksum == expected;

    if !verified {
        println!(
            "invalid checksum: \n\
            found:    {checksum}\n\
            expected: {expected}",
        );
    }

    verified
}

fn unpack(exec_ctx: &ExecutionContext, tarball: &Path, dst: &Path, pattern: &str) -> PathBuf {
    eprintln!("extracting {} to {}", tarball.display(), dst.display());
    if !dst.exists() {
        t!(fs::create_dir_all(dst));
    }

    // `tarball` ends with `.tar.xz`; strip that suffix
    // example: `rust-dev-nightly-x86_64-unknown-linux-gnu`
    let uncompressed_filename =
        Path::new(tarball.file_name().expect("missing tarball filename")).file_stem().unwrap();
    let directory_prefix = Path::new(Path::new(uncompressed_filename).file_stem().unwrap());

    // decompress the file
    let data = t!(File::open(tarball), format!("file {} not found", tarball.display()));
    let decompressor = XzDecoder::new(BufReader::new(data));

    let mut tar = tar::Archive::new(decompressor);

    let is_ci_rustc = dst.ends_with("ci-rustc");
    let is_ci_llvm = dst.ends_with("ci-llvm");

    // `compile::Sysroot` needs to know the contents of the `rustc-dev` tarball to avoid adding
    // it to the sysroot unless it was explicitly requested. But parsing the 100 MB tarball is slow.
    // Cache the entries when we extract it so we only have to read it once.
    let mut recorded_entries = if is_ci_rustc { recorded_entries(dst, pattern) } else { None };

    for member in t!(tar.entries()) {
        let mut member = t!(member);
        let original_path = t!(member.path()).into_owned();
        // skip the top-level directory
        if original_path == directory_prefix {
            continue;
        }
        let mut short_path = t!(original_path.strip_prefix(directory_prefix));
        let is_builder_config = short_path.to_str() == Some(BUILDER_CONFIG_FILENAME);

        if !(short_path.starts_with(pattern) || ((is_ci_rustc || is_ci_llvm) && is_builder_config))
        {
            continue;
        }
        short_path = short_path.strip_prefix(pattern).unwrap_or(short_path);
        let dst_path = dst.join(short_path);

        exec_ctx.do_if_verbose(|| {
            println!("extracting {} to {}", original_path.display(), dst.display());
        });

        if !t!(member.unpack_in(dst)) {
            panic!("path traversal attack ??");
        }
        if let Some(record) = &mut recorded_entries {
            t!(writeln!(record, "{}", short_path.to_str().unwrap()));
        }
        let src_path = dst.join(original_path);
        if src_path.is_dir() && dst_path.exists() {
            continue;
        }
        t!(move_file(src_path, dst_path));
    }
    let dst_dir = dst.join(directory_prefix);
    if dst_dir.exists() {
        t!(fs::remove_dir_all(&dst_dir), format!("failed to remove {}", dst_dir.display()));
    }
    dst.to_path_buf()
}

fn download_file<'a>(
    dwn_ctx: impl AsRef<DownloadContext<'a>>,
    out: &Path,
    url: &str,
    dest_path: &Path,
    help_on_error: &str,
) {
    let dwn_ctx = dwn_ctx.as_ref();

    dwn_ctx.exec_ctx.do_if_verbose(|| {
        println!("download {url}");
    });
    // Use a temporary file in case we crash while downloading, to avoid a corrupt download in cache/.
    let tempfile = tempdir(out).join(dest_path.file_name().unwrap());
    // While bootstrap itself only supports http and https downloads, downstream forks might
    // need to download components from other protocols. The match allows them adding more
    // protocols without worrying about merge conflicts if we change the HTTP implementation.
    match url.split_once("://").map(|(proto, _)| proto) {
        Some("http") | Some("https") => download_http_with_retries(
            dwn_ctx.host_target,
            dwn_ctx.is_running_on_ci(),
            dwn_ctx.exec_ctx,
            &tempfile,
            url,
            help_on_error,
        ),
        Some(other) => panic!("unsupported protocol {other} in {url}"),
        None => panic!("no protocol in {url}"),
    }
    t!(move_file(&tempfile, dest_path), format!("failed to rename {tempfile:?} to {dest_path:?}"));
}

/// Create a temporary directory in `out` and return its path.
///
/// NOTE: this temporary directory is shared between all steps;
/// if you need an empty directory, create a new subdirectory inside it.
pub(crate) fn tempdir(out: &Path) -> PathBuf {
    let tmp = out.join("tmp");
    t!(fs::create_dir_all(&tmp));
    tmp
}

fn download_http_with_retries(
    host_target: TargetSelection,
    is_running_on_ci: bool,
    exec_ctx: &ExecutionContext,
    tempfile: &Path,
    url: &str,
    help_on_error: &str,
) {
    println!("downloading {url}");
    assert!(cfg!(not(test)), "unit tests shouldn't be downloading things: {url:?}");

    // Try curl. If that fails and we are on windows, fallback to PowerShell.
    // options should be kept in sync with
    // src/bootstrap/src/core/download.rs
    // for consistency
    let mut curl = command("curl").allow_failure();
    curl.args([
        // follow redirect
        "--location",
        // timeout if speed is < 10 bytes/sec for > 30 seconds
        "--speed-time",
        "30",
        "--speed-limit",
        "10",
        // timeout if cannot connect within 30 seconds
        "--connect-timeout",
        "30",
        // output file
        "--output",
        tempfile.to_str().unwrap(),
        // if there is an error, don't restart the download,
        // instead continue where it left off.
        "--continue-at",
        "-",
        // retry up to 3 times.  note that this means a maximum of 4
        // attempts will be made, since the first attempt isn't a *re*try.
        "--retry",
        "3",
        // show errors, even if --silent is specified
        "--show-error",
        // set timestamp of downloaded file to that of the server
        "--remote-time",
        // fail on non-ok http status
        "--fail",
    ]);
    // Don't print progress in CI; the \r wrapping looks bad and downloads don't take long enough for progress to be useful.
    if is_running_on_ci {
        curl.arg("--silent");
    } else {
        curl.arg("--progress-bar");
    }
    // --retry-all-errors was added in 7.71.0, don't use it if curl is old.
    if curl_version(exec_ctx) >= semver::Version::new(7, 71, 0) {
        curl.arg("--retry-all-errors");
    }
    curl.arg(url);
    if !curl.run(exec_ctx) {
        if host_target.contains("windows-msvc") {
            eprintln!("Fallback to PowerShell");
            for _ in 0..3 {
                let powershell = command("PowerShell.exe").allow_failure().args([
                    "/nologo",
                    "-Command",
                    "[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12;",
                    &format!(
                        "(New-Object System.Net.WebClient).DownloadFile('{}', '{}')",
                        url, tempfile.to_str().expect("invalid UTF-8 not supported with powershell downloads"),
                    ),
                ]).run_capture_stdout(exec_ctx);

                if powershell.is_success() {
                    return;
                }

                eprintln!("\nspurious failure, trying again");
            }
        }
        if !help_on_error.is_empty() {
            eprintln!("{help_on_error}");
        }
        helpers::exit_process(1);
    }
}

fn curl_version(exec_ctx: &ExecutionContext) -> semver::Version {
    let mut curl = command("curl");
    curl.arg("-V");
    let curl = curl.run_capture_stdout(exec_ctx);
    if curl.is_failure() {
        return semver::Version::new(1, 0, 0);
    }
    let output = curl.stdout();
    extract_curl_version(output)
}
