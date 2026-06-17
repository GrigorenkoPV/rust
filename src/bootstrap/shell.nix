{
  pkgs ? import <nixpkgs> { },
}:
pkgs.mkShell {
  name = "rustc-bootstrap";
  __structuredAttrs = true;
  strictDeps = true;
}
