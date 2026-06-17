{
  lib,
  rustPlatform,
}:
let
  manifest = lib.fromTOML (lib.readFile ./Cargo.toml);
in
rustPlatform.buildRustPackage (finalAttrs: {
  pname = manifest.package.name;
  version = manifest.package.version;

  src = lib.fileset.toSource {
    root = ./..;
    fileset = lib.fileset.unions (
      lib.concatMap ({ dir, files }: lib.map (lib.path.append dir) files) [
        {
          dir = ./.;
          files = [
            "src"
            "Cargo.lock"
            "Cargo.toml"
            "build.rs"
          ];
        }
        {
          dir = ./..;
          files = [
            "build_helper"
            "shim_utils"
            "stage0"
          ];
        }
        {
          dir = ../etc;
          files = [
            "rust_analyzer_eglot.el"
            "rust_analyzer_helix.toml"
            "rust_analyzer_settings.json"
            "rust_analyzer_zed.json"
          ];
        }
      ]
    );
  };
  sourceRoot = "source/bootstrap";

  cargoLock = {
    lockFile = ./Cargo.lock;
  };

  buildType = "debug";
  doCheck = false;

  strictDeps = true;
  __structuredAttrs = true;
  enableParallelBuilding = true;

  meta = {
    description = "Helper for rust-lang/rust x.py";
    homepage = "https://github.com/rust-lang/rust/blob/HEAD/src/bootstrap";
    license = lib.licenses.mit;
    mainProgram = manifest.package.name;
  };
})
