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
          toolchain = fenix.packages.${system}.stable.withComponents [
            "cargo"
            "rustc"
            "rustfmt"
            "clippy"
            "rust-src"
          ];
          craneLib = (crane.mkLib pkgs).overrideToolchain toolchain;
          schemaFilter =
            path: type:
            (type == "regular" || type == "directory")
            && (builtins.match ".*/schema(/.*)?" path != null);
          sourceFilter =
            path: type:
            (craneLib.filterCargoSources path type) || (schemaFilter path type);
          src = pkgs.lib.cleanSourceWith {
            src = ./.;
            filter = sourceFilter;
            name = "source";
          };
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
              pname = "terminal";
              meta.mainProgram = "terminal";
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
            name = "terminal-test-named-session-registry";
            runtimeInputs = [
              context.pkgs.coreutils
              context.pkgs.gnugrep
            ];
            text = ''
              export TERMINAL_PACKAGE=${package}
              export TERMINAL_BASH=${context.pkgs.bash}/bin/bash
              ${context.pkgs.bash}/bin/bash ${./scripts/named-session-registry-witness}
            '';
          };
          terminalSignalWitness = context.pkgs.writeShellApplication {
            name = "terminal-test-terminal-signal";
            runtimeInputs = [
              context.pkgs.coreutils
              context.pkgs.gnugrep
            ];
            text = ''
              export TERMINAL_PACKAGE=${package}
              export TERMINAL_BASH=${context.pkgs.bash}/bin/bash
              ${context.pkgs.bash}/bin/bash ${./scripts/terminal-signal-witness}
            '';
          };
          gateCacheWitness = context.pkgs.writeShellApplication {
            name = "terminal-test-gate-cache";
            runtimeInputs = [
              context.pkgs.coreutils
              context.pkgs.gawk
              context.pkgs.gnugrep
            ];
            text = ''
              export TERMINAL_PACKAGE=${package}
              export TERMINAL_BASH=${context.pkgs.bash}/bin/bash
              ${context.pkgs.bash}/bin/bash ${./scripts/gate-cache-witness}
            '';
          };
          dirtyPromptDefersWitness = context.pkgs.writeShellApplication {
            name = "terminal-test-dirty-prompt-defers";
            runtimeInputs = [
              context.pkgs.coreutils
              context.pkgs.gawk
              context.pkgs.gnugrep
            ];
            text = ''
              export TERMINAL_PACKAGE=${package}
              export TERMINAL_BASH=${context.pkgs.bash}/bin/bash
              ${context.pkgs.bash}/bin/bash ${./scripts/dirty-prompt-defers-witness}
            '';
          };
        in
        {
          default = {
            type = "app";
            program = "${package}/bin/terminal";
          };
          daemon = {
            type = "app";
            program = "${package}/bin/terminal-daemon";
          };
          view = {
            type = "app";
            program = "${package}/bin/terminal";
          };
          send = {
            type = "app";
            program = "${package}/bin/terminal-send";
          };
          capture = {
            type = "app";
            program = "${package}/bin/terminal-capture";
          };
          type = {
            type = "app";
            program = "${package}/bin/terminal-type";
          };
          sessions = {
            type = "app";
            program = "${package}/bin/terminal-sessions";
          };
          resolve = {
            type = "app";
            program = "${package}/bin/terminal-resolve";
          };
          signal = {
            type = "app";
            program = "${package}/bin/terminal-signal";
          };
          supervisor = {
            type = "app";
            program = "${package}/bin/terminal-supervisor";
          };
          # This witness allocates a host PTY, so it is an app instead of a
          # pure Nix builder check.
          test-named-session-registry = {
            type = "app";
            program = "${namedSessionRegistryWitness}/bin/terminal-test-named-session-registry";
          };
          test-terminal-signal = {
            type = "app";
            program = "${terminalSignalWitness}/bin/terminal-test-terminal-signal";
          };
          test-gate-cache = {
            type = "app";
            program = "${gateCacheWitness}/bin/terminal-test-gate-cache";
          };
          test-dirty-prompt-defers = {
            type = "app";
            program = "${dirtyPromptDefersWitness}/bin/terminal-test-dirty-prompt-defers";
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
          terminal-supervisor-applies-spawn-envelope-socket-mode =
            context.craneLib.cargoTest (
              context.commonArgs
              // {
                inherit (context) cargoArtifacts;
                cargoTestExtraArgs = "--test terminal_supervisor terminal_supervisor_daemon_applies_spawn_envelope_socket_mode -- --exact";
              }
            );
          terminal-supervisor-answers-component-supervision-relation =
            context.craneLib.cargoTest (
              context.commonArgs
              // {
                inherit (context) cargoArtifacts;
                cargoTestExtraArgs = "--test terminal_supervisor terminal_supervisor_answers_component_supervision_relation -- --exact";
              }
            );
          terminal-registration-writes-session-health = context.craneLib.cargoTest (
            context.commonArgs
            // {
              inherit (context) cargoArtifacts;
              cargoTestExtraArgs = "--test terminal_registry terminal_daemon_registration_writes_named_session -- --exact";
            }
          );
          terminal-supervisor-socket-routes-through-component-sema = context.craneLib.cargoTest (
            context.commonArgs
            // {
              inherit (context) cargoArtifacts;
              cargoTestExtraArgs = "--test terminal_supervisor terminal_supervisor_socket_routes_through_component_sema";
            }
          );
          terminal-supervisor-subscription-streams-initial-state-then-delta =
            context.craneLib.cargoTest (
              context.commonArgs
              // {
                inherit (context) cargoArtifacts;
                cargoTestExtraArgs = "--test terminal_supervisor terminal_supervisor_subscription_streams_initial_state_then_delta";
              }
            );
          terminal-supervisor-uses-spawn-envelope-environment = context.craneLib.cargoTest (
            context.commonArgs
            // {
              inherit (context) cargoArtifacts;
              cargoTestExtraArgs = "--test terminal_supervisor terminal_supervisor_command_line_uses_spawn_envelope_environment";
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
