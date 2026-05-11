{
  description = "Persona terminal session owner built around terminal-cell.";

  inputs = {
    nixpkgs.url = "github:LiGoldragon/nixpkgs?ref=main";

    fenix.url = "github:nix-community/fenix";
    fenix.inputs.nixpkgs.follows = "nixpkgs";

    crane.url = "github:ipetkov/crane";
  };

  outputs =
    { self, nixpkgs, fenix, crane }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forSystems = function: nixpkgs.lib.genAttrs systems (system: function system);

      mkContext =
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          toolchain = fenix.packages.${system}.fromToolchainFile {
            file = ./rust-toolchain.toml;
            sha256 = "sha256-gh/xTkxKHL4eiRXzWv8KP7vfjSk61Iq48x47BEDFgfk=";
          };
          craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
          src = craneLib.cleanCargoSource ./.;
          cargoVendorDir = craneLib.vendorCargoDeps { inherit src; };
          commonArgs = {
            inherit src cargoVendorDir;
            strictDeps = true;
          };
          cargoArtifacts = craneLib.buildDepsOnly commonArgs;
        in
        {
          inherit pkgs toolchain craneLib commonArgs cargoArtifacts;
        };
    in
    {
      packages = forSystems (
        system:
        let
          context = mkContext system;
        in
        {
          default = context.craneLib.buildPackage (
            context.commonArgs
            // {
              inherit (context) cargoArtifacts;
              pname = "persona-terminal";
              meta.mainProgram = "persona-terminal-view";
            }
          );
        }
      );

      apps = forSystems (
        system:
        let
          context = mkContext system;
          package = self.packages.${system}.default;
          namedSessionRegistryWitness = context.pkgs.writeShellApplication {
            name = "persona-terminal-test-named-session-registry";
            runtimeInputs = [
              context.pkgs.coreutils
              context.pkgs.gnugrep
            ];
            text = ''
              export PERSONA_TERMINAL_PACKAGE=${package}
              export PERSONA_TERMINAL_BASH=${context.pkgs.bash}/bin/bash
              ${context.pkgs.bash}/bin/bash ${./scripts/named-session-registry-witness}
            '';
          };
          terminalSignalWitness = context.pkgs.writeShellApplication {
            name = "persona-terminal-test-terminal-signal";
            runtimeInputs = [
              context.pkgs.coreutils
              context.pkgs.gnugrep
            ];
            text = ''
              export PERSONA_TERMINAL_PACKAGE=${package}
              export PERSONA_TERMINAL_BASH=${context.pkgs.bash}/bin/bash
              ${context.pkgs.bash}/bin/bash ${./scripts/terminal-signal-witness}
            '';
          };
        in
        {
          default = {
            type = "app";
            program = "${package}/bin/persona-terminal-view";
          };
          daemon = {
            type = "app";
            program = "${package}/bin/persona-terminal-daemon";
          };
          view = {
            type = "app";
            program = "${package}/bin/persona-terminal-view";
          };
          send = {
            type = "app";
            program = "${package}/bin/persona-terminal-send";
          };
          capture = {
            type = "app";
            program = "${package}/bin/persona-terminal-capture";
          };
          type = {
            type = "app";
            program = "${package}/bin/persona-terminal-type";
          };
          sessions = {
            type = "app";
            program = "${package}/bin/persona-terminal-sessions";
          };
          resolve = {
            type = "app";
            program = "${package}/bin/persona-terminal-resolve";
          };
          signal = {
            type = "app";
            program = "${package}/bin/persona-terminal-signal";
          };
          # This witness allocates a host PTY, so it is an app instead of a
          # pure Nix builder check.
          test-named-session-registry = {
            type = "app";
            program = "${namedSessionRegistryWitness}/bin/persona-terminal-test-named-session-registry";
          };
          test-terminal-signal = {
            type = "app";
            program = "${terminalSignalWitness}/bin/persona-terminal-test-terminal-signal";
          };
        }
      );

      checks = forSystems (
        system:
        let
          context = mkContext system;
        in
        {
          default = context.craneLib.cargoTest (
            context.commonArgs
            // {
              inherit (context) cargoArtifacts;
            }
          );
        }
      );

      devShells = forSystems (
        system:
        let
          context = mkContext system;
        in
        {
          default = context.pkgs.mkShell {
            packages = [
              context.toolchain
              context.pkgs.jujutsu
              context.pkgs.nix
            ];
          };
        }
      );
    };
}
