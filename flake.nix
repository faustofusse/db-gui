{
  description = "dbear dev shell: Rust core (+ SwiftUI on macOS, GPUI on Linux)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { nixpkgs, flake-utils, rust-overlay, ... }:
    flake-utils.lib.eachSystem [ "aarch64-darwin" "x86_64-darwin" "x86_64-linux" "aarch64-linux" ] (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };
        inherit (pkgs) lib stdenv;

        # Same toolchain rustup would pick: channel, components and targets from rust-toolchain.toml.
        rust = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;

        # Native libraries GPUI needs on Linux (window system, GPU, fonts).
        linuxLibs = with pkgs; [
          wayland
          libxkbcommon
          vulkan-loader
          libGL
          fontconfig
          freetype
          openssl
          alsa-lib
          xorg.libX11
          xorg.libxcb
          xorg.libXcursor
          xorg.libXi
          xorg.libXrandr
        ];

        common = with pkgs; [
          rust
          pkg-config
        ];
      in
      {
        devShells.default =
          if stdenv.hostPlatform.isDarwin then
            # No Nix C compiler or Apple SDK here on purpose: Swift, swift build, xcodebuild,
            # lipo and codesign must come from the installed Xcode. Nix's cc wrapper and SDKROOT
            # would clash with them. Cargo and the cc crate use Xcode's clang too (rust-overlay
            # otherwise brings in Nix's clang wrapper as `cc`).
            pkgs.mkShellNoCC {
              packages = common;
              shellHook = ''
                unset SDKROOT DEVELOPER_DIR
                export PATH="$PATH:/usr/bin"
                export CC=/usr/bin/clang CXX=/usr/bin/clang++ AR=/usr/bin/ar
                export CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER=/usr/bin/clang
                export CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER=/usr/bin/clang
                if ! /usr/bin/xcrun --find swift >/dev/null 2>&1; then
                  echo "warning: Xcode not found; the macOS app needs Xcode installed (xcode-select -p)." >&2
                fi
              '';
            }
          else
            pkgs.mkShell {
              packages = common ++ [ pkgs.clang pkgs.mold ];
              buildInputs = linuxLibs;
              LD_LIBRARY_PATH = lib.makeLibraryPath linuxLibs;
            };

        formatter = pkgs.nixfmt-rfc-style;
      });
}
