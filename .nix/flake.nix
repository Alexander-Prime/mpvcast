{
  description = "mpvcast - REST API server for remote mpv control";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    revdiff.url = "github:umputun/revdiff";
    claude-code.url = "github:sadjow/claude-code-nix";
  };

  outputs = { self, nixpkgs, flake-utils, revdiff, claude-code }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
      in
      {
        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            cargo
            rustc
            rustfmt
            rust-analyzer
            clippy

            # Runtime dependencies
            mpv
            yt-dlp

            # Diff review
            revdiff.packages.${system}.default
            jq

            claude-code.packages.${system}.default
          ];

          # Set rust-analyzer to use the Nix-provided toolchain
          RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
        };
      }
    );
}
