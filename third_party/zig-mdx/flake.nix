{
  description = "zig-mdx - MDX tokenizer/parser in Zig";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    zig = {
      url = "github:mitchellh/zig-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, zig }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        inherit (pkgs) lib stdenv;
        zigPkg = zig.packages.${system}."0.15.1";
        devInputs = with pkgs; [
          zigPkg
          git
        ] ++ lib.optionals stdenv.isLinux [
          gcc
        ] ++ lib.optionals stdenv.isDarwin [
          clang
        ];
      in {
        devShells.default = pkgs.mkShell {
          buildInputs = devInputs;
          shellHook = ''
            export ZIG_GLOBAL_CACHE_DIR=$(pwd)/.zig-cache
            export ZIG_LOCAL_CACHE_DIR=$ZIG_GLOBAL_CACHE_DIR
            export PATH="${zigPkg}/bin:$PATH"
            if [ -n "''${NIX_CFLAGS_COMPILE-}" ]; then
              filtered_flags=""
              for flag in $NIX_CFLAGS_COMPILE; do
                case "$flag" in
                  -fmacro-prefix-map=*) ;;
                  *) filtered_flags="$filtered_flags $flag" ;;
                esac
              done
              NIX_CFLAGS_COMPILE="''${filtered_flags# }"
              export NIX_CFLAGS_COMPILE
            fi
            ${lib.optionalString stdenv.isDarwin ''
              # Allow Zig to find macOS system frameworks
              export NIX_ENFORCE_PURITY=0
              if [ -z "''${SDKROOT:-}" ]; then
                export SDKROOT=$(xcrun --show-sdk-path 2>/dev/null || echo "/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk")
              fi
            ''}
            echo "zig-mdx development environment"
            echo "Zig ${zigPkg.version}"
          '';
        };
      }
    );
}
