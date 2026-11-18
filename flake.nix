{
  description = "dictum development tools";

  inputs.nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

  outputs = { nixpkgs, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      # Rust itself comes from rustup (rust-toolchain.toml). This shell adds the
      # tools the checks in `justfile` and `.github/workflows` call.
      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            just
            cargo-deny
            cargo-nextest
            cargo-fuzz
            typos
            taplo
            actionlint
            elan
            tlaplus
          ];
        };
      });
    };
}
